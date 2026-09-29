//! The address clients reach this server at, for links that leave it.

use axum::http::{HeaderMap, header};

/// `sharing.public_url` when set; otherwise the Host the request came in on,
/// with the scheme a proxy in front says it terminated. koan itself serves
/// plain HTTP, so without a forwarded scheme the request was plain HTTP.
pub fn origin(headers: &HeaderMap, public_url: Option<&str>) -> Option<String> {
    if let Some(url) = public_url.map(str::trim).filter(|u| !u.is_empty()) {
        return Some(url.trim_end_matches('/').to_owned());
    }
    let value = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let host = value("x-forwarded-host")
        .or_else(|| value(header::HOST.as_str()))?
        .split(',')
        .next()?
        .trim();
    let scheme = value("x-forwarded-proto")
        .and_then(|p| p.split(',').next())
        .map(str::trim)
        .filter(|p| matches!(*p, "http" | "https"))
        .unwrap_or("http");
    (!host.is_empty()).then(|| format!("{scheme}://{host}"))
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
    fn the_configured_address_wins() {
        let h = headers(&[("host", "10.0.0.2:4000")]);
        assert_eq!(
            origin(&h, Some("https://music.example.com/")).as_deref(),
            Some("https://music.example.com")
        );
    }

    #[test]
    fn a_proxy_supplies_the_scheme_and_host() {
        let h = headers(&[
            ("host", "koan:4000"),
            ("x-forwarded-host", "music.example.com"),
            ("x-forwarded-proto", "https"),
        ]);
        assert_eq!(
            origin(&h, None).as_deref(),
            Some("https://music.example.com")
        );
    }

    #[test]
    fn a_direct_request_is_plain_http() {
        let h = headers(&[("host", "192.168.1.5:4000")]);
        assert_eq!(
            origin(&h, Some("")).as_deref(),
            Some("http://192.168.1.5:4000")
        );
        assert_eq!(origin(&HeaderMap::new(), None), None);
    }
}
