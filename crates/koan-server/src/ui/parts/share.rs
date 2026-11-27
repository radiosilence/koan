//! Share links: a record, cued to a track or not, or an artist, made into a
//! link anyone can open, shown in the page's share region with a button that
//! copies it.

use askama::Template;
use axum::response::Response;
use koan_core::helpers::ShareTarget;
use koan_kit_web as kit;

use crate::auth::AuthUser;
use crate::html::{Html, Render};
use crate::share::blocking;
use crate::ui::{UiState, answer, may_edit, open};

/// The id of the region a page that shares shows the link in.
pub(in crate::ui) const ID: &str = "share";

/// The share region: empty, the link made, or why it could not be.
#[derive(Template)]
#[template(path = "parts/share.html")]
pub(in crate::ui) struct Shared {
    result: Option<Result<String, String>>,
}

impl Shared {
    /// The region as a page draws it, before anything is shared.
    pub(in crate::ui) fn empty() -> Html {
        Self { result: None }.html()
    }

    fn id(&self) -> &'static str {
        ID
    }
}

/// Make a share link and show it in the page's share region.
pub(in crate::ui) async fn make(s: UiState, user: AuthUser, target: ShareTarget) -> Response {
    let result = if may_edit(&user) {
        blocking(move || {
            let db = open(&s.pool)?;
            let cfg = koan_core::config::Config::cached();
            Some(
                koan_core::helpers::create_native_share(&db, user.user_id, &cfg, &target, None)
                    .map(|o| o.url)
                    .map_err(|e| e.to_string()),
            )
        })
        .await
        .unwrap_or_else(|| Err("The library is unavailable.".into()))
    } else {
        Err("This account cannot make share links.".into())
    };
    answer([Shared {
        result: Some(result),
    }
    .html()])
}
