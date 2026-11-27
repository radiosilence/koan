//! Approving a device that is waiting to be signed in, for someone without
//! the app: type the code the device shows, or follow its link here. See
//! `crate::pair`.
//!
//! Plain forms, laid out like the sign-in page, since the page is reached from
//! a phone that may never have opened the UI. They prove their origin the way
//! the OAuth consent form does.

use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;

use askama::Template;
use koan_kit_web as kit;

use super::{cross_site, framed, same_origin};
use crate::auth::AuthUser;
use crate::html::Render;
use crate::share::blocking;
use crate::ui::{UiState, open, paths, see_other};

const LIMITED: &str =
    "Too many codes were tried that no device was waiting with. Try again in a minute.";

enum View<'a> {
    Code {
        error: Option<&'a str>,
    },
    NoAccounts,
    Confirm {
        device: &'a str,
        from: String,
        local: bool,
        user: &'a str,
        pair: &'a str,
    },
    Declined {
        device: &'a str,
    },
    Approved {
        device: &'a str,
        user: &'a str,
    },
    Message {
        heading: &'a str,
        text: &'a str,
    },
}

#[derive(Template)]
#[template(path = "sign_in/pair.html")]
struct PairPage<'a> {
    view: View<'a>,
}

impl PairPage<'_> {
    fn code_form(&self) -> kit::Form<'static> {
        kit::Form::plain_get(paths::pair())
    }

    /// The code as it is typed, formatted in place, the caret kept among the
    /// same symbols. A copy of koan-core's `remote::pair::format_code`; the
    /// two change together.
    fn code(&self) -> kit::Field<'static> {
        kit::Field::new(
            "code",
            kit::Input::text("code")
                .placeholder("XXXX-XXXX")
                .autocomplete("off")
                .attr("autocapitalize", "characters")
                .attr("spellcheck", "false")
                .attr("maxlength", "20")
                .required()
                .autofocus()
                .datastar(
                    "on:input",
                    "let at = el.value.slice(0, el.selectionStart).replace(/[^0-9a-z]/gi, '').length; el.value = el.value.toUpperCase().replace(/[IL]/g, '1').replace(/O/g, '0').replace(/[^0-9A-HJKMNP-TV-Z]/g, '').slice(0, 8).replace(/^.{4}(?=.)/, (m) => m + '-'); at = Math.min(at > 4 ? at + 1 : at, el.value.length); el.setSelectionRange(at, at)",
                ),
        )
    }

    fn approve(&self, pair: &str) -> kit::Form<'static> {
        kit::Form::plain(paths::pair_approve(pair))
    }

    fn decline(&self, pair: &str) -> kit::Form<'static> {
        kit::Form::plain(paths::pair_decline(pair))
    }

    fn sign_out(&self) -> kit::Form<'static> {
        kit::Form::plain(paths::sign_out())
    }

    fn here(&self, pair: &str) -> String {
        format!("/{}", paths::pairing(pair))
    }

    fn not_me(&self, user: &str) -> kit::Button<'static> {
        kit::Button::quiet(format!("Not {user}? Sign out")).submit()
    }
}

fn page(s: &UiState, status: StatusCode, view: View<'_>) -> Response {
    framed(
        s,
        status,
        "Sign in a device",
        PairPage { view }.html(),
        false,
    )
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
pub(in crate::ui) struct CodeQuery {
    code: String,
}

/// The code form. Submitted, it goes to the pairing it names.
pub(in crate::ui) async fn form(State(s): State<UiState>, Query(q): Query<CodeQuery>) -> Response {
    let code = q.code.trim();
    if !code.is_empty() {
        return see_other(&format!("/{}", paths::pairing(code)));
    }
    code_page(&s, None)
}

fn code_page(s: &UiState, error: Option<&str>) -> Response {
    if !s.auth_enabled {
        return without_accounts(s);
    }
    let status = if error.is_none() {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    };
    page(s, status, View::Code { error })
}

fn without_accounts(s: &UiState) -> Response {
    page(s, StatusCode::OK, View::NoAccounts)
}

/// "Sign in this device as you?", for the pairing an id or a code names.
pub(in crate::ui) async fn confirm(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(pair): Path<String>,
) -> Response {
    if !s.auth_enabled {
        return without_accounts(&s);
    }
    let info = match crate::pair::pairings().info(&pair, user.user_id) {
        Ok(info) => info,
        Err(crate::pair::PairError::Limited) => return code_page(&s, Some(LIMITED)),
        Err(_) => {
            return code_page(
                &s,
                Some(
                    "No device is waiting with that code. Codes last ten minutes: check the device for a new one.",
                ),
            );
        }
    };
    let view = View::Confirm {
        device: &info.device,
        from: info.from.to_string(),
        local: info.local(),
        user: &user.username,
        pair: &pair,
    };
    page(&s, StatusCode::OK, view)
}

pub(in crate::ui) async fn approve(
    state: State<UiState>,
    user: Extension<AuthUser>,
    headers: HeaderMap,
    pair: Path<String>,
) -> Response {
    settle(state, user, headers, pair, false).await
}

pub(in crate::ui) async fn decline(
    state: State<UiState>,
    user: Extension<AuthUser>,
    headers: HeaderMap,
    pair: Path<String>,
) -> Response {
    settle(state, user, headers, pair, true).await
}

async fn settle(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
    Path(pair): Path<String>,
    decline: bool,
) -> Response {
    if !same_origin(&headers) {
        return cross_site();
    }
    if !s.auth_enabled {
        return without_accounts(&s);
    }
    let username = user.username.clone();
    let pool = s.pool.clone();
    let settled = blocking(move || {
        let db = open(&pool)?;
        Some(crate::pair::pairings().settle(&db.conn, &pair, user.user_id, &user.username, decline))
    })
    .await;
    let view = match &settled {
        Some(Ok(info)) if decline => View::Declined {
            device: &info.device,
        },
        Some(Ok(info)) => View::Approved {
            device: &info.device,
            user: &username,
        },
        Some(Err(crate::pair::PairError::Limited)) => View::Message {
            heading: "Wait a minute",
            text: LIMITED,
        },
        Some(Err(crate::pair::PairError::NotFound)) => View::Message {
            heading: "Nothing to sign in",
            text: "The device stopped waiting, or its code lapsed. Check the device for a new code.",
        },
        Some(Err(crate::pair::PairError::Internal(_))) | None => View::Message {
            heading: "Something went wrong",
            text: "The device could not be signed in. Try again.",
        },
    };
    page(&s, StatusCode::OK, view)
}

#[cfg(test)]
mod tests {
    use crate::ui::fixture::*;

#[tokio::test]
async fn a_waiting_device_is_approved_from_the_pair_page() {
    let f = setup(true);
    let r = send(&f.app, get("/pair/ABCD-EFGH").body(Body::empty()).unwrap()).await;
    assert_eq!(r.location(), "/auth/resume?next=%2Fpair%2FABCD-EFGH");

    let opened = crate::pair::pairings()
        .open("Living <room> TV", None, "10.0.0.8".parse().unwrap())
        .unwrap();
    let typed = opened.code.to_lowercase();
    let device = crate::pair::device(opened, true);
    let r = send(
        &f.app,
        authed(&f.state, &format!("/pair?code={typed}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(r.location(), format!("/pair/{typed}"));
    let r = send(
        &f.app,
        authed(&f.state, r.location()).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(
        r.body
            .contains("Sign in <span class=\"normal-case\">Living &#60;room&#62; TV</span>?"),
        "{}",
        r.body
    );
    assert!(r.body.contains("<strong>alice</strong>"));
    assert!(r.body.contains("Requested from 10.0.0.8, on your network."));
    let far = crate::pair::pairings()
        .open("Far TV", None, "2001:db8::7".parse().unwrap())
        .unwrap();
    let r = send(
        &f.app,
        authed(&f.state, &format!("/pair/{}", far.code))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert!(
        r.body
            .contains("Requested from 2001:db8::7, from the internet."),
        "{}",
        r.body
    );
    drop(far);

    let post = |origin: &str| {
        Request::post(format!("/pair/{typed}/approve"))
            .header(header::HOST, HOST)
            .header(header::ORIGIN, origin)
            .header(
                header::COOKIE,
                format!("koan_access={}", access_token(&f.state)),
            )
            .body(Body::empty())
            .unwrap()
    };
    let r = send(&f.app, post("https://evil.test")).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = send(&f.app, post(ORIGIN)).await;
    assert!(r.body.contains("Signed in"), "{}", r.body);
    let db = Database::open(&f.dir.path().join("koan.db")).unwrap();
    let keys = queries::api_keys::list_api_keys(&db.conn, Some(1)).unwrap();
    assert_eq!(keys[0].name, "Living <room> TV");

    // Settled: the code no longer names anything.
    let r = send(&f.app, post(ORIGIN)).await;
    assert!(r.body.contains("Nothing to sign in"));
    device.join().unwrap();
}
}
