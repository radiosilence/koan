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
use axum::response::{IntoResponse, Response};

use super::{UiState, encode, html, open, see_other};
use crate::auth::AuthUser;
use crate::html::Render;
use crate::share::blocking;
use askama::Template;

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
        action: String,
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
#[template(path = "pages/pair.html")]
struct PairPage<'a> {
    title: &'a str,
    view: View<'a>,
}

fn page(status: StatusCode, view: View<'_>) -> Response {
    let page = PairPage {
        title: "Sign in a device",
        view,
    };
    html(status, page.html())
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
pub(super) struct CodeQuery {
    code: String,
}

/// The code form. Submitted, it goes to the pairing it names.
pub(super) async fn form(State(s): State<UiState>, Query(q): Query<CodeQuery>) -> Response {
    let code = q.code.trim();
    if !code.is_empty() {
        return see_other(&format!("/pair/{}", encode(code)));
    }
    code_page(&s, None)
}

fn code_page(s: &UiState, error: Option<&str>) -> Response {
    if !s.auth_enabled {
        return without_accounts();
    }
    let status = if error.is_none() {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    };
    page(status, View::Code { error })
}

fn without_accounts() -> Response {
    page(StatusCode::OK, View::NoAccounts)
}

/// "Sign in this device as you?", for the pairing an id or a code names.
pub(super) async fn confirm(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(pair): Path<String>,
) -> Response {
    if !s.auth_enabled {
        return without_accounts();
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
        action: format!("/pair/{}", encode(&pair)),
    };
    page(StatusCode::OK, view)
}

pub(super) async fn approve(
    state: State<UiState>,
    user: Extension<AuthUser>,
    headers: HeaderMap,
    pair: Path<String>,
) -> Response {
    settle(state, user, headers, pair, false).await
}

pub(super) async fn decline(
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
    if !super::session::same_origin(&headers) {
        return (StatusCode::FORBIDDEN, "cross-site request refused").into_response();
    }
    if !s.auth_enabled {
        return without_accounts();
    }
    let username = user.username.clone();
    let settled = blocking(move || {
        let db = open(&s.pool)?;
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
    page(StatusCode::OK, view)
}
