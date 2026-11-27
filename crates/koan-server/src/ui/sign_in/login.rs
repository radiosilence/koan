//! The sign-in page: a password, or a passkey on its own.

use askama::Template;
use axum::Form;
use axum::extract::{Query, State};
use axum::http::{Extensions, HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use koan_kit_web as kit;
use serde::Deserialize;

use super::{
    NextParam, Vouch, cross_site, framed, local_path, same_origin, second_step, signed_in, vouched,
};
use crate::auth::routes::{ClientIp, SignedIn, authenticate, passkey_sign_in};
use crate::html::Render;
use crate::ui::{UiState, encode, paths, see_other};

/// The sign-in form, with what went wrong last time.
#[derive(Template)]
#[template(path = "sign_in/login.html")]
struct Login<'a> {
    next: &'a str,
    error: Option<&'a str>,
    /// On a demo server, the shared account, filled in.
    demo: Option<(&'a str, &'a str)>,
    /// Whether a passkey can sign in here: not where the server's address
    /// allows none. The page's script asks for a challenge as it loads.
    passkey: bool,
}

impl Login<'_> {
    fn form(&self) -> kit::Form<'static> {
        kit::Form::plain(paths::login())
    }

    fn username(&self) -> kit::Field<'static> {
        let mut input = kit::Input::text("username")
            .autocomplete(if self.passkey {
                "username webauthn"
            } else {
                "username"
            })
            .attr("autocapitalize", "none")
            .attr("spellcheck", "false")
            .required()
            .autofocus();
        if let Some((name, _)) = self.demo {
            input = input.value(name.to_owned());
        }
        kit::Field::new("username", input)
    }

    fn password(&self) -> kit::Field<'static> {
        let mut input = kit::Input::password("password")
            .autocomplete("current-password")
            .required();
        if let Some((_, password)) = self.demo {
            input = input.value(password.to_owned());
        }
        kit::Field::new("password", input)
    }

    fn passkey_form(&self) -> kit::Form<'static> {
        kit::Form::plain(paths::login_passkey())
    }

    fn challenge(&self) -> &'static str {
        paths::login_passkey_challenge()
    }
}

/// The sign-in page, whole.
pub(super) fn page(s: &UiState, status: StatusCode, next: &str, error: Option<&str>) -> Response {
    let passkey = s.relying_party().is_some();
    let view = Login {
        next,
        error,
        demo: crate::demo::enabled().then_some((crate::demo::USERNAME, crate::demo::PASSWORD)),
        passkey,
    };
    framed(s, status, "Sign in", view.html(), passkey)
}

pub(in crate::ui) async fn form(
    State(s): State<UiState>,
    Query(q): Query<NextParam>,
    headers: HeaderMap,
    ext: Extensions,
) -> Response {
    let next = local_path(&q.next);
    if !s.auth_enabled {
        return see_other(next);
    }
    if vouched(&s, &headers, &ext) != Vouch::Absent {
        return see_other(&format!("{}?next={}", paths::PROXY_RESUME, encode(next)));
    }
    // Nobody to sign in as yet: make the admin first.
    if super::setup::open_for_setup(&s).await.is_some() {
        return see_other(paths::SETUP);
    }
    page(&s, StatusCode::OK, next, None)
}

#[derive(Deserialize)]
pub(in crate::ui) struct LoginForm {
    username: String,
    password: String,
    #[serde(default)]
    next: String,
}

pub(in crate::ui) async fn login(
    State(s): State<UiState>,
    ClientIp(from): ClientIp,
    headers: HeaderMap,
    ext: Extensions,
    Form(f): Form<LoginForm>,
) -> Response {
    if !same_origin(&headers) {
        return cross_site();
    }
    let next = local_path(&f.next);
    // Through the proxy, the proxy says who is signed in, not a password.
    if vouched(&s, &headers, &ext) != Vouch::Absent {
        return see_other(&format!("{}?next={}", paths::PROXY_RESUME, encode(next)));
    }
    match authenticate(&s.auth, &f.username, &f.password, None, from).await {
        Ok(SignedIn::Session(_, access, refresh)) => signed_in(&s, next, &access, &refresh),
        Ok(SignedIn::NeedsSecond { user, .. }) => {
            use koan_auth::second_factor::Purpose;
            match s
                .auth
                .users
                .tickets()
                .open(user.id, &user.username, Purpose::SignIn)
            {
                Ok((ticket, held)) => second_step::page(&s, next, &ticket, &held, None).await,
                Err(_) => page(
                    &s,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    next,
                    Some("Signing in failed. Try again."),
                ),
            }
        }
        Err(resp) => {
            let status = resp.status();
            let message = match status {
                StatusCode::UNAUTHORIZED => "Wrong username or password.",
                StatusCode::TOO_MANY_REQUESTS => {
                    "Too many failed sign-ins for this account. Try again in a minute."
                }
                _ => "Signing in failed. Try again.",
            };
            page(&s, status, next, Some(message))
        }
    }
}

#[derive(Deserialize)]
pub(in crate::ui) struct PasskeyForm {
    ticket: String,
    #[serde(default)]
    next: String,
    assertion: String,
}

/// A challenge for a passkey to sign in with on its own: the ticket that
/// holds it and `navigator.credentials.get`'s options. Asked by the sign-in
/// page's script, under a per-address limit, so that loading the page
/// stores nothing and nobody can crowd out others' challenges by loading it.
pub(in crate::ui) async fn passkey_challenge(
    State(s): State<UiState>,
    headers: HeaderMap,
) -> Response {
    use koan_auth::second_factor::Purpose;
    if !same_origin(&headers) {
        return cross_site();
    }
    let Some(rp) = s.relying_party().filter(|_| s.auth_enabled) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match s.auth.users.passkeys().open(0, "", Purpose::Passkey) {
        Ok((ticket, held)) => (
            [(header::CACHE_CONTROL, "no-store")],
            axum::Json(serde_json::json!({
                "ticket": ticket,
                "options": koan_auth::webauthn::passkey_options(&rp, &held.challenge),
            })),
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// A passkey signing in on its own, from the sign-in page.
pub(in crate::ui) async fn passkey(
    State(s): State<UiState>,
    ClientIp(from): ClientIp,
    headers: HeaderMap,
    Form(f): Form<PasskeyForm>,
) -> Response {
    if !same_origin(&headers) {
        return cross_site();
    }
    let next = local_path(&f.next);
    let (Some(rp), Ok(assertion)) = (s.relying_party(), serde_json::from_str(&f.assertion)) else {
        return page(
            &s,
            StatusCode::BAD_REQUEST,
            next,
            Some("Signing in failed. Try again."),
        );
    };
    match passkey_sign_in(&s.auth, f.ticket, assertion, rp, from).await {
        Ok((_, access, refresh)) => signed_in(&s, next, &access, &refresh),
        Err(resp) => {
            let message = match resp.status() {
                StatusCode::UNAUTHORIZED => {
                    "That passkey does not sign in to this server, or did not check it was you."
                }
                StatusCode::GONE => "That took too long. Try again.",
                _ => "Signing in failed. Try again.",
            };
            page(&s, resp.status(), next, Some(message))
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ui::fixture::*;

    #[tokio::test]
    async fn the_sign_in_page_offers_passkeys_only_where_browsers_allow_them() {
        let f = setup_at(true, Some("https://music.example.org"));
        let r = send(&f.app, get("/login").body(Body::empty()).unwrap()).await;
        assert!(r.body.contains("data-passkey-login"), "{}", r.body);
        assert!(r.body.contains("username webauthn"));
        let r = send(&f.app, form("/login/passkey/challenge", "")).await;
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v["options"]["userVerification"], "required", "{}", r.body);
        assert_eq!(v["options"]["rpId"], "music.example.org");
        assert!(v["ticket"].as_str().is_some_and(|t| !t.is_empty()));

        let f = setup(true);
        let r = send(&f.app, get("/login").body(Body::empty()).unwrap()).await;
        assert!(!r.body.contains("data-passkey-login"));
        let r = send(&f.app, form("/login/passkey/challenge", "")).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND);
    }
}
