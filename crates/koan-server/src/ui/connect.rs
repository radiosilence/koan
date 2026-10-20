//! How to connect an AI assistant to this server's `/mcp`. A browser opening
//! `/mcp` is sent here.

use axum::Extension;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;

use askama::Template;

use super::UiState;
use super::pages::respond;
use crate::auth::AuthUser;

/// `url` is the address to give an assistant, absent while the server does
/// not know its own.
#[derive(Template)]
#[template(path = "pages/connect.html")]
struct ConnectPage<'a> {
    url: Option<String>,
    admin: bool,
    username: &'a str,
    abilities: &'a str,
    days: u64,
}

pub(super) async fn page(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    let inner = ConnectPage {
        url: super::oauth::public_base(&s).map(|base| format!("{base}/mcp")),
        admin: user.role == koan_core::auth::Role::Admin,
        username: &user.username,
        abilities: super::oauth::abilities(&user),
        days: s.auth.refresh_ttl_secs / 86_400,
    }
    .render()
    .unwrap_or_default();
    respond(&s, &headers, &user, "Assistants", &inner)
}
