//! How to connect an AI assistant to this server's `/mcp`. A browser opening
//! `/mcp` is sent here.

use axum::Extension;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;

use super::UiState;
use super::pages::respond;
use crate::auth::AuthUser;
use crate::share::escape;

pub(super) async fn page(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    let intro = "<h1>Assistants</h1><p class=sub>Claude, ChatGPT and other assistants that speak \
MCP can search your library, make playlists and play music on your devices, as you.</p>";
    let Some(base) = super::oauth::public_base(&s) else {
        let fix = if user.role == koan_core::auth::Role::Admin {
            "Set <code>sharing.public_url</code> to the address this server is reached at"
        } else {
            "Its admin needs to set the address this server is reached at"
        };
        let inner = format!(
            "{intro}<p class=empty>Assistants sign in through this server, so it needs to know \
its own address first. {fix}.</p>"
        );
        return respond(&s, &headers, &user, "Assistants", "connect", &inner);
    };
    let days = s.auth.refresh_ttl_secs / 86_400;
    let inner = format!(
        "{intro}<div class=share><input id=mcp-url readonly value=\"{url}\" aria-label=\"MCP address\">\
<button data-copy=mcp-url>Copy</button></div>\
<h2>Claude</h2><ol><li>Settings → Connectors → Add custom connector.</li>\
<li>Paste the address above.</li>\
<li>Claude opens a kōan page. Sign in if asked, check it says <em>Connect kōan to claude.ai</em>, \
and Allow.</li></ol>\
<h2>Other assistants</h2><p>Any MCP client that supports OAuth: add the address as a remote MCP \
server and approve it the same way. The approval page names where the connection goes; approve \
only one you started yourself.</p>\
<h2>What it can do</h2><p>It acts as {name}: it can {abilities}.</p>\
<h2>Disconnecting</h2><p>Remove the connector in the assistant. A connection unused for {days} days \
ends by itself, and changing your password ends all of yours.</p>",
        url = escape(&format!("{base}/mcp")),
        name = escape(&user.username),
        abilities = super::oauth::abilities(&user),
    );
    respond(&s, &headers, &user, "Assistants", "connect", &inner)
}
