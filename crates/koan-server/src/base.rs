//! Serving under a path: `https://host/koan` rather than a host of its own.
//!
//! The path is `server.url`'s (`share.url`'s for an app's server). Routes and
//! handlers never see it: [`edge`] takes it off each request before routing and
//! puts it back on what a response names by path, its `Location` and its
//! cookies' `Path`. A proxy may forward the path or strip it; both arrive at
//! the same routes. Pages carry it as `<base href>` and link relatively.

use axum::extract::{Request, State};
use axum::http::{HeaderValue, Uri, header};
use axum::middleware::Next;
use axum::response::Response;

use crate::ServerUrl;

/// The path part of a server address, without its trailing slash: `/koan`
/// for `https://host/koan/`, empty for a server at the root.
pub(crate) fn path_of(url: Option<&str>) -> String {
    let Some(url) = url else {
        return String::new();
    };
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    match rest.find('/') {
        Some(at) => rest[at..].trim_end_matches('/').to_owned(),
        None => String::new(),
    }
}

/// What a page's `<base href>` names: the base path with its slash.
pub(crate) fn href(url: Option<&str>) -> String {
    format!("{}/", path_of(url))
}

impl ServerUrl {
    pub(crate) fn base(&self) -> String {
        path_of(self.get().as_deref())
    }
}

/// Wraps the whole router, outside routing, so the routes match unprefixed.
pub(crate) async fn edge(State(url): State<ServerUrl>, mut req: Request, next: Next) -> Response {
    let base = url.base();
    if base.is_empty() {
        return next.run(req).await;
    }
    if let Some(uri) = strip(req.uri(), &base) {
        *req.uri_mut() = uri;
    }
    let mut response = next.run(req).await;
    restore(&mut response, &base);
    response
}

/// The request's address without `base`, or `None` when it lacks it, as it
/// does through a proxy that strips the path.
fn strip(uri: &Uri, base: &str) -> Option<Uri> {
    let rest = uri.path().strip_prefix(base)?;
    let path = match rest {
        "" => "/",
        rest if rest.starts_with('/') => rest,
        _ => return None,
    };
    let mut parts = uri.clone().into_parts();
    let path_and_query = match uri.query() {
        Some(q) => format!("{path}?{q}"),
        None => path.to_owned(),
    };
    parts.path_and_query = Some(path_and_query.parse().ok()?);
    Uri::from_parts(parts).ok()
}

fn restore(response: &mut Response, base: &str) {
    let headers = response.headers_mut();
    if let Some(location) = headers.get(header::LOCATION).and_then(|v| v.to_str().ok())
        && location.starts_with('/')
        && !location.starts_with("//")
        && let Ok(v) = HeaderValue::from_str(&format!("{base}{location}"))
    {
        headers.insert(header::LOCATION, v);
    }
    let cookies: Vec<HeaderValue> = headers
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| {
            v.to_str()
                .ok()
                .and_then(|c| HeaderValue::from_str(&cookie_path(c, base)).ok())
                .unwrap_or_else(|| v.clone())
        })
        .collect();
    if !cookies.is_empty() {
        headers.remove(header::SET_COOKIE);
        for c in cookies {
            headers.append(header::SET_COOKIE, c);
        }
    }
}

/// A cookie scoped to `/auth` becomes one scoped to `/koan/auth`; one scoped
/// to the root, to `/koan`, which matches `/koan` and everything under it.
fn cookie_path(cookie: &str, base: &str) -> String {
    cookie
        .split(';')
        .map(|attr| {
            let trimmed = attr.trim_start();
            match trimmed.get(..5) {
                Some(name) if name.eq_ignore_ascii_case("path=") => {
                    let path = &trimmed[5..];
                    let lead = &attr[..attr.len() - trimmed.len()];
                    match path {
                        "/" => format!("{lead}Path={base}"),
                        p if p.starts_with('/') => format!("{lead}Path={base}{p}"),
                        p => format!("{lead}Path={p}"),
                    }
                }
                _ => attr.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join(";")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::routing::get;
    use tower::ServiceExt;

    #[test]
    fn the_path_comes_from_the_address() {
        assert_eq!(path_of(None), "");
        assert_eq!(path_of(Some("https://host")), "");
        assert_eq!(path_of(Some("https://host/")), "");
        assert_eq!(path_of(Some("https://host/koan")), "/koan");
        assert_eq!(path_of(Some("https://host/koan/")), "/koan");
        assert_eq!(path_of(Some("https://host:8443/a/b?x=1")), "/a/b");
        assert_eq!(href(Some("https://host")), "/");
        assert_eq!(href(Some("https://host/koan")), "/koan/");
    }

    #[test]
    fn cookies_are_scoped_under_the_path() {
        assert_eq!(
            cookie_path("r=1; HttpOnly; Path=/auth; Max-Age=3", "/koan"),
            "r=1; HttpOnly; Path=/koan/auth; Max-Age=3"
        );
        assert_eq!(cookie_path("s=1; Path=/", "/koan"), "s=1; Path=/koan");
        assert_eq!(cookie_path("s=1; Secure", "/koan"), "s=1; Secure");
    }

    fn app() -> axum::Router {
        axum::Router::new()
            .route("/", get(|| async { "root" }))
            .route(
                "/go",
                get(|uri: Uri| async move {
                    (
                        axum::http::StatusCode::SEE_OTHER,
                        [
                            (header::LOCATION, "/login?next=/go".to_owned()),
                            (header::SET_COOKIE, "a=1; Path=/auth".to_owned()),
                        ],
                        uri.to_string(),
                    )
                }),
            )
    }

    async fn call(base: &str, path: &str) -> Response {
        let url = ServerUrl::Fixed(Some(format!("https://host{base}")));
        let wrapped = axum::middleware::from_fn_with_state(url, edge).layer(app());
        wrapped
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn text(r: Response) -> String {
        let bytes = axum::body::to_bytes(r.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn a_forwarded_path_and_a_stripped_one_reach_the_same_route() {
        for path in ["/koan/go?x=1", "/go?x=1"] {
            let r = call("/koan", path).await;
            assert_eq!(r.headers()[header::LOCATION], "/koan/login?next=/go");
            assert_eq!(r.headers()[header::SET_COOKIE], "a=1; Path=/koan/auth");
            assert_eq!(text(r).await, "/go?x=1");
        }
        assert_eq!(text(call("/koan", "/koan").await).await, "root");
        assert_eq!(text(call("/koan", "/koan/").await).await, "root");
    }

    #[tokio::test]
    async fn a_neighbouring_path_is_not_taken_for_the_base() {
        let r = call("/koan", "/koanx/go").await;
        assert_eq!(r.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn at_the_root_nothing_changes() {
        let r = call("", "/go").await;
        assert_eq!(r.headers()[header::LOCATION], "/login?next=/go");
        assert_eq!(r.headers()[header::SET_COOKIE], "a=1; Path=/auth");
    }
}
