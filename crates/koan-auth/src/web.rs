//! What a server's sign-in pages need to check about a request.

use http::{HeaderMap, header};

/// Whether a request came from a page on the same origin, the way a browser
/// says so: `Sec-Fetch-Site`, or an `Origin` naming the host the request was
/// sent to. A plain form cannot carry a custom header, so this is what stands
/// between a sign-in form's POST and another site's.
pub fn same_origin(headers: &HeaderMap) -> bool {
    let get = |name: header::HeaderName| headers.get(name).and_then(|v| v.to_str().ok());
    if get(header::HeaderName::from_static("sec-fetch-site")) == Some("same-origin") {
        return true;
    }
    match (get(header::ORIGIN), get(header::HOST)) {
        (Some(origin), Some(host)) => origin
            .split_once("://")
            .is_some_and(|(_, authority)| authority.eq_ignore_ascii_case(host)),
        _ => false,
    }
}

/// `next`, if it is a path on this server: anything else (`//evil.example`,
/// `https://…`) would make a sign-in page an open redirect. Browsers read a
/// backslash as a slash and drop tabs and newlines from a URL before parsing
/// it, so `/\t/evil.example` is `//evil.example`: any such character, or any
/// whitespace or non-ASCII at all, refuses the path, rather than predicting
/// what a browser makes of it.
pub fn local_path(next: &str) -> Option<&str> {
    let local = next.starts_with('/')
        && !next.starts_with("//")
        && next.bytes().all(|b| b.is_ascii_graphic() && b != b'\\');
    local.then_some(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, v.parse().unwrap());
        }
        h
    }

    #[test]
    fn only_this_origin_is_same_origin() {
        assert!(same_origin(&headers(&[("sec-fetch-site", "same-origin")])));
        assert!(!same_origin(&headers(&[("sec-fetch-site", "cross-site")])));
        assert!(same_origin(&headers(&[
            ("origin", "https://app.example"),
            ("host", "app.example")
        ])));
        assert!(!same_origin(&headers(&[
            ("origin", "https://evil.example"),
            ("host", "app.example")
        ])));
        assert!(!same_origin(&headers(&[])));
    }

    #[test]
    fn only_a_local_path_is_returned_to() {
        assert_eq!(local_path("/albums/abc?x=1"), Some("/albums/abc?x=1"));
        for sneaky in [
            "//evil.example/x",
            "https://evil.example",
            "/\\evil.example",
            "/\t/evil.example",
            "/\n/evil.example",
            "/\r/evil.example",
            "/ /evil.example",
            "/\u{0}/x",
            "",
        ] {
            assert_eq!(local_path(sneaky), None, "{sneaky:?}");
        }
    }
}
