//! OAuth's consent page: whether to let an assistant reach `/mcp` as the
//! account signed in. The client is named by the host it returns to, since a
//! client may give itself any name. A plain form, since it answers with a
//! redirect to the client; it proves its origin the way the sign-in form
//! does.

use askama::Template;
use axum::Extension;
use axum::Form;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use koan_auth::oauth::{Authorizing, Server};
use koan_kit_web as kit;

use super::{Frame, cross_site, same_origin};
use crate::auth::AuthUser;
use crate::html::Render;
use crate::share::not_found;
use crate::ui::oauth::{AuthorizeParams, abilities, server};
use crate::ui::{UiState, document_with, paths};

/// Why an authorization request is not answered.
#[derive(Template)]
#[template(path = "sign_in/refused.html")]
struct Refused<'a> {
    why: &'a str,
}

/// The consent page. `fields` carries the request on to the approval, and
/// `here` brings the person back to it after signing in as someone else.
#[derive(Template)]
#[template(path = "sign_in/consent.html")]
struct Consent<'a> {
    host: &'a str,
    name: &'a str,
    abilities: &'a str,
    user: &'a str,
    fields: [(&'a str, &'a str); 8],
    here: String,
}

impl Consent<'_> {
    fn form(&self) -> kit::Form<'static> {
        kit::Form::plain(paths::authorize())
    }

    fn sign_out(&self) -> kit::Form<'static> {
        kit::Form::plain(paths::sign_out())
    }

    fn allow(&self) -> kit::Button<'static> {
        kit::Button::primary("Allow")
            .named("decision", "allow")
            .submit()
    }

    fn deny(&self) -> kit::Button<'static> {
        kit::Button::quiet("Deny")
            .named("decision", "deny")
            .submit()
    }

    fn not_me(&self) -> kit::Button<'static> {
        kit::Button::quiet(format!("Not {}? Sign out", self.user)).submit()
    }
}

/// A page outside the shell under `policy`.
fn page(
    s: &UiState,
    status: StatusCode,
    title: &str,
    body: crate::html::Html,
    policy: askama_datastar::Csp,
) -> Response {
    let frame = Frame {
        base: s.base(),
        title,
        body,
        passkey: false,
    };
    document_with(status, &frame.html(), policy.header_value())
}

/// An authorization request this server will answer, or the page saying why
/// not: never a redirect until the client and its redirect URI hold, or this
/// would bounce users to any address.
fn check(s: &UiState, q: &AuthorizeParams) -> Result<(Server, Authorizing), Box<Response>> {
    let server = server(s).ok_or_else(|| Box::new(not_found()))?;
    match server.check(&s.auth.public_pem, q) {
        Ok(authorizing) => Ok((server, authorizing)),
        Err(why) => Err(Box::new(page(
            s,
            StatusCode::BAD_REQUEST,
            "Cannot connect",
            Refused { why }.html(),
            crate::ui::csp(),
        ))),
    }
}

/// Send the browser back to the client with `pairs`.
fn answer(
    server: &Server,
    to: &Authorizing,
    q: &AuthorizeParams,
    pairs: &[(&str, &str)],
) -> Response {
    (
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, server.answer(&to.to, q, pairs)),
            (header::CACHE_CONTROL, "no-store".into()),
        ],
    )
        .into_response()
}

pub(in crate::ui) async fn authorize(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<AuthorizeParams>,
) -> Response {
    let (server, to) = match check(&s, &q) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if let Some((error, why)) = server.request_error(&q) {
        return answer(
            &server,
            &to,
            &q,
            &[("error", error), ("error_description", why)],
        );
    }
    let view = Consent {
        host: to.to.host_str().unwrap_or_default(),
        name: &to.client.client_name,
        abilities: abilities(&user),
        user: &user.username,
        fields: q.fields(),
        here: format!("{}?{}", paths::AUTHORIZE, q.query()),
    };
    // The approval answers with a redirect to the client, which a browser
    // holds the form's `form-action` to as well.
    let policy = crate::ui::csp().allow("form-action", to.to.origin().ascii_serialization());
    page(&s, StatusCode::OK, "Connect an app", view.html(), policy)
}

pub(in crate::ui) async fn approve(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
    Form(q): Form<AuthorizeParams>,
) -> Response {
    if !same_origin(&headers) {
        return cross_site();
    }
    let (server, to) = match check(&s, &q) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if let Some((error, why)) = server.request_error(&q) {
        return answer(
            &server,
            &to,
            &q,
            &[("error", error), ("error_description", why)],
        );
    }
    if q.decision != "allow" {
        return answer(&server, &to, &q, &[("error", "access_denied")]);
    }
    let Ok(code) = s.codes.issue(&q, &to, user.user_id) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    answer(&server, &to, &q, &[("code", &code)])
}
