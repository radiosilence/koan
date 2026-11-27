//! Assistants: how to connect Claude or another assistant to this server's
//! `/mcp`. A browser opening `/mcp` is sent here.

use askama::Template;

use super::{Body, Page, Refusal, Section};
use crate::auth::AuthUser;
use crate::ui::{UiState, is_admin, oauth};
use koan_kit_web as kit;

pub(in crate::ui) struct Assistants;

/// `url` is the address to give an assistant, absent while the server does
/// not know its own.
#[derive(Template)]
#[template(path = "assistants/page.html")]
struct AssistantsPage<'a> {
    url: Option<String>,
    admin: bool,
    username: &'a str,
    abilities: &'a str,
    days: u64,
}

impl Page for Assistants {
    const SECTION: Section = Section::Account;
    type Args = ();

    async fn body(s: &UiState, user: &AuthUser, (): ()) -> Result<Body, Refusal> {
        let view = AssistantsPage {
            url: oauth::mcp_url(s),
            admin: is_admin(user),
            username: &user.username,
            abilities: oauth::abilities(user),
            days: s.auth.refresh_ttl_secs / 86_400,
        };
        Ok(Body::of("Assistants", &view))
    }
}
