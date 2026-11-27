//! A sign-in's second step, its password passed: a security key, or a code
//! from an authenticator app, or a recovery code.

use askama::Template;
use axum::Form;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use koan_kit_web as kit;
use serde::Deserialize;

use super::{cross_site, framed, local_path, same_origin, signed_in};
use crate::auth::routes::{Answer, ClientIp, finish_second};
use crate::html::Render;
use crate::ui::{Logged, UiState, encode, open, paths};

/// The second step: `ticket` holds the sign-in, and with the account's
/// security keys, the challenge they answer.
#[derive(Template)]
#[template(path = "sign_in/second_step.html")]
struct SecondStep<'a> {
    next: &'a str,
    ticket: &'a str,
    totp: bool,
    /// `navigator.credentials.get`'s options, as JSON.
    key_options: Option<&'a str>,
    error: Option<&'a str>,
}

impl SecondStep<'_> {
    fn form(&self) -> kit::Form<'static> {
        kit::Form::plain(paths::login_second())
    }

    fn code(&self) -> kit::Field<'static> {
        let label = if self.totp {
            "code from your authenticator app"
        } else {
            "recovery code"
        };
        let mut input = kit::Input::text("code")
            .autocomplete("one-time-code")
            .attr("autocapitalize", "none")
            .attr("spellcheck", "false")
            .required();
        if self.totp || self.key_options.is_none() {
            input = input.autofocus();
        }
        kit::Field::new(label, input).hint(if self.totp {
            "Lost the app? A recovery code works here too."
        } else {
            "Lost your key? Type one of your recovery codes."
        })
    }

    fn continue_button(&self) -> kit::Button<'static> {
        if self.key_options.is_none() {
            kit::Button::primary("Continue")
        } else {
            kit::Button::standard("Continue")
        }
    }

    fn key_button(&self) -> kit::Button<'static> {
        let b = kit::Button::primary("Use a security key or passkey");
        if self.totp { b } else { b.autofocus() }
    }

    fn someone_else(&self) -> String {
        format!("{}?next={}", paths::login(), encode(self.next))
    }
}

/// The page asking for the second factor. A wrong answer draws it again on
/// the same ticket, so its tries and its lifetime run on.
pub(super) async fn page(
    s: &UiState,
    next: &str,
    ticket: &str,
    held: &koan_auth::second_factor::Ticket,
    error: Option<&str>,
) -> Response {
    let (pool, user_id) = (s.pool.clone(), held.user_id);
    let (totp, ids) = tokio::task::spawn_blocking(move || {
        let db = open(&pool)?;
        let factors = koan_auth::second_factor::factors(&db.conn, user_id).logged()?;
        let ids = koan_auth::second_factor::key_ids(&db.conn, user_id).logged()?;
        Some((factors.totp, ids))
    })
    .await
    .ok()
    .flatten()
    .unwrap_or_default();
    let key_options = s
        .relying_party()
        .filter(|_| !ids.is_empty())
        .map(|rp| koan_auth::webauthn::request_options(&rp, &held.challenge, &ids).to_string());
    let view = SecondStep {
        next,
        ticket,
        totp,
        key_options: key_options.as_deref(),
        error,
    };
    let status = if error.is_some() {
        StatusCode::UNAUTHORIZED
    } else {
        StatusCode::OK
    };
    framed(s, status, "Sign in", view.html(), true)
}

#[derive(Deserialize)]
pub(in crate::ui) struct SecondForm {
    ticket: String,
    #[serde(default)]
    next: String,
    #[serde(default)]
    code: String,
    /// A security key's answer, as the page's script wrote it.
    #[serde(default)]
    assertion: String,
}

/// The second factor for a sign-in whose password passed.
pub(in crate::ui) async fn second(
    State(s): State<UiState>,
    ClientIp(from): ClientIp,
    headers: HeaderMap,
    Form(f): Form<SecondForm>,
) -> Response {
    if !same_origin(&headers) {
        return cross_site();
    }
    let next = local_path(&f.next);
    let answer = if f.assertion.is_empty() {
        Answer::Code(f.code)
    } else {
        match serde_json::from_str(&f.assertion) {
            Ok(assertion) => Answer::Key(assertion),
            Err(_) => Answer::Code(String::new()),
        }
    };
    let ticket = f.ticket.clone();
    match finish_second(&s.auth, f.ticket, answer, s.relying_party(), from).await {
        Ok((_, access, refresh)) => signed_in(&s, next, &access, &refresh),
        Err(resp) => {
            let status = resp.status();
            let message = match status {
                StatusCode::UNAUTHORIZED => "That was not right. Try again.",
                StatusCode::TOO_MANY_REQUESTS => {
                    "Too many wrong answers for this account. Try again in a minute."
                }
                StatusCode::LOCKED => {
                    "Too many wrong codes in a row. Sign in with a passkey, or ask an admin to turn two-step sign-in off."
                }
                StatusCode::CONFLICT => "This server's address takes no security keys. Use a code.",
                StatusCode::GONE => {
                    return super::login::page(
                        &s,
                        StatusCode::UNAUTHORIZED,
                        next,
                        Some("That took too long or too many tries. Sign in again."),
                    );
                }
                _ => "Signing in failed. Try again.",
            };
            // Again on the same ticket, while it has tries left; once it
            // has none, the password is asked again.
            let held = s
                .auth
                .users
                .tickets()
                .get(&ticket, koan_auth::second_factor::Purpose::SignIn);
            match held {
                Some(held)
                    if status != StatusCode::TOO_MANY_REQUESTS && status != StatusCode::LOCKED =>
                {
                    page(&s, next, &ticket, &held, Some(message)).await
                }
                _ => super::login::page(&s, status, next, Some(message)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ui::fixture::*;

    #[tokio::test]
    async fn two_step_sign_in_asks_for_a_code_after_the_password() {
        let f = setup(true);
        let db = Database::open(f.state.pool.path()).unwrap();
        let key = koan_auth::second_factor::totp_key(&f.state.private_pem);
        let secret = koan_auth::second_factor::begin_totp(&db.conn, &key, 1).unwrap();
        db.conn
            .execute("UPDATE totp SET confirmed = 1 WHERE user_id = 1", [])
            .unwrap();
        let code = || {
            let step = koan_auth::totp::step(koan_core::clock::unix_secs());
            format!("{:06}", koan_auth::totp::hotp(&secret, step as u64))
        };

        // The password alone opens no session: the second step's page.
        let r = send(
            &f.app,
            form("/login", "username=alice&password=hunter2&next=%2Fqueue"),
        )
        .await;
        assert_eq!(r.status, StatusCode::OK);
        assert!(r.body.contains("two-step sign-in"), "{}", r.body);
        assert!(r.cookies().is_empty());
        let ticket = input_value(&r.body, "ticket").unwrap();

        let r = send(
            &f.app,
            form(
                "/login/second",
                &format!("ticket={ticket}&code=000000&next=%2Fqueue"),
            ),
        )
        .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
        assert!(r.cookies().is_empty());
        // The same ticket again, its tries running on.
        assert!(r.body.contains(&format!("value=\"{ticket}\"")));
        let ticket = input_value(&r.body, "ticket").unwrap();

        let r = send(
            &f.app,
            form(
                "/login/second",
                &format!("ticket={ticket}&code={}&next=%2Fqueue", code()),
            ),
        )
        .await;
        assert_eq!((r.status, r.location()), (StatusCode::SEE_OTHER, "/queue"));
        assert!(!r.cookie("koan_access").is_empty());

        // The ticket answered once.
        let r = send(
            &f.app,
            form("/login/second", &format!("ticket={ticket}&code={}", code())),
        )
        .await;
        assert!(r.cookies().is_empty());
    }
}
