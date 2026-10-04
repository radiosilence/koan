//! SOAP actions: an envelope out, the action's out-arguments or a fault back.

use std::time::Duration;

use super::description::Service;
use super::xml;

/// Long enough for a renderer that is switching inputs, short enough that the
/// player thread waiting on it is never stuck for long.
const TIMEOUT: Duration = Duration::from_secs(4);

#[derive(Debug, thiserror::Error)]
pub enum SoapError {
    /// No answer at all: refused, unroutable or timed out. A renderer that
    /// was switched off looks like this.
    #[error("not answering: {0}")]
    Unreachable(String),
    #[error("{0}")]
    Http(String),
    #[error("renderer refused {action}: {code} {description}")]
    Fault {
        action: String,
        code: u32,
        description: String,
    },
    #[error("unreadable reply to {action}: {reason}")]
    Malformed { action: String, reason: String },
}

/// No idle connections are kept. Renderers' HTTP servers are small and many
/// close a connection without saying so; a request sent down one of those is
/// lost, and a SOAP call is rare enough that a fresh connection costs nothing.
pub fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .pool_max_idle_per_host(0)
        .timeout(TIMEOUT)
        .connect_timeout(Duration::from_secs(2))
        .build()
        .expect("reqwest client builds")
}

pub fn envelope(service_type: &str, action: &str, args: &[(&str, &str)]) -> String {
    let mut body = String::new();
    for (name, value) in args {
        body.push_str(&format!("<{name}>{}</{name}>", xml::escape(value)));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">\
<s:Body><u:{action} xmlns:u=\"{service_type}\">{body}</u:{action}></s:Body></s:Envelope>"
    )
}

/// Run `action` and return its out-arguments by name.
pub fn call(
    http: &reqwest::blocking::Client,
    service: &Service,
    action: &str,
    args: &[(&str, &str)],
) -> Result<Vec<(String, String)>, SoapError> {
    let response = http
        .post(service.control.clone())
        .header("Content-Type", "text/xml; charset=\"utf-8\"")
        .header(
            "SOAPACTION",
            format!("\"{}#{action}\"", service.service_type),
        )
        .body(envelope(&service.service_type, action, args))
        .send()
        .map_err(|e| {
            if e.is_connect() || e.is_timeout() {
                SoapError::Unreachable(e.to_string())
            } else {
                SoapError::Http(e.to_string())
            }
        })?;
    let status = response.status();
    let text = response
        .text()
        .map_err(|e| SoapError::Http(e.to_string()))?;
    parse_response(action, status.as_u16(), &text)
}

pub fn parse_response(
    action: &str,
    status: u16,
    text: &str,
) -> Result<Vec<(String, String)>, SoapError> {
    let malformed = |reason: String| SoapError::Malformed {
        action: action.to_string(),
        reason,
    };
    let root = match xml::parse(text) {
        Ok(root) => root,
        Err(_) if status >= 400 => return Err(SoapError::Http(format!("HTTP {status}"))),
        Err(e) => return Err(malformed(e)),
    };
    if let Some(fault) = root.find("Fault") {
        let error = fault.find("UPnPError");
        return Err(SoapError::Fault {
            action: action.to_string(),
            code: error
                .and_then(|e| e.child_text("errorCode"))
                .and_then(|c| c.parse().ok())
                .unwrap_or(0),
            description: error
                .and_then(|e| e.child_text("errorDescription"))
                .or_else(|| fault.child_text("faultstring"))
                .unwrap_or_default()
                .to_string(),
        });
    }
    if status >= 400 {
        return Err(SoapError::Http(format!("HTTP {status}")));
    }
    let reply = root
        .child("Body")
        .and_then(|b| b.children.first())
        .ok_or_else(|| malformed("no body".into()))?;
    Ok(reply
        .children
        .iter()
        .map(|c| (c.name.clone(), c.text.clone()))
        .collect())
}

/// Look up one out-argument.
pub fn arg<'a>(args: &'a [(String, String)], name: &str) -> Option<&'a str> {
    args.iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

/// `H+:MM:SS[.F+]` (the AVTransport time format) to milliseconds. `None` for
/// `NOT_IMPLEMENTED` and anything else that is not a time.
pub fn parse_time(s: &str) -> Option<u64> {
    let s = s.trim();
    let (clock, frac) = match s.split_once('.') {
        Some((clock, frac)) => (clock, Some(frac)),
        None => (s, None),
    };
    let mut parts = clock.split(':').rev();
    let secs: u64 = parts.next()?.parse().ok()?;
    let mins: u64 = parts.next().unwrap_or("0").parse().ok()?;
    let hours: u64 = parts.next().unwrap_or("0").parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let ms = match frac {
        // A fraction is either decimal or `F0/F1`; only the decimal is worth
        // reading, and only to milliseconds.
        Some(f) if f.chars().all(|c| c.is_ascii_digit()) && !f.is_empty() => {
            let digits: String = f.chars().chain("000".chars()).take(3).collect();
            digits.parse().unwrap_or(0)
        }
        _ => 0,
    };
    Some(((hours * 60 + mins) * 60 + secs) * 1000 + ms)
}

pub fn format_time(ms: u64) -> String {
    let secs = ms / 1000;
    format!("{}:{:02}:{:02}", secs / 3600, (secs / 60) % 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_escapes_arguments() {
        let env = envelope(
            "urn:schemas-upnp-org:service:AVTransport:1",
            "SetAVTransportURI",
            &[("CurrentURIMetaData", "<DIDL-Lite>&</DIDL-Lite>")],
        );
        assert!(env.contains(
            "<CurrentURIMetaData>&lt;DIDL-Lite&gt;&amp;&lt;/DIDL-Lite&gt;</CurrentURIMetaData>"
        ));
        assert!(env.contains(
            "<u:SetAVTransportURI xmlns:u=\"urn:schemas-upnp-org:service:AVTransport:1\">"
        ));
    }

    #[test]
    fn out_arguments_are_read_by_name() {
        let reply = r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><u:GetPositionInfoResponse xmlns:u="urn:schemas-upnp-org:service:AVTransport:1"><Track>1</Track><TrackDuration>0:03:12.000</TrackDuration><TrackURI>http://x/t/a.flac</TrackURI><RelTime>0:01:02.5</RelTime></u:GetPositionInfoResponse></s:Body></s:Envelope>"#;
        let args = parse_response("GetPositionInfo", 200, reply).unwrap();
        assert_eq!(arg(&args, "TrackURI"), Some("http://x/t/a.flac"));
        assert_eq!(arg(&args, "RelTime").and_then(parse_time), Some(62_500));
    }

    #[test]
    fn a_fault_carries_the_upnp_error() {
        let reply = r#"<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><s:Fault><faultcode>s:Client</faultcode><faultstring>UPnPError</faultstring><detail><UPnPError xmlns="urn:schemas-upnp-org:control-1-0"><errorCode>714</errorCode><errorDescription>Illegal MIME-type</errorDescription></UPnPError></detail></s:Fault></s:Body></s:Envelope>"#;
        match parse_response("SetAVTransportURI", 500, reply) {
            Err(SoapError::Fault {
                code, description, ..
            }) => {
                assert_eq!(code, 714);
                assert_eq!(description, "Illegal MIME-type");
            }
            other => panic!("expected a fault, got {other:?}"),
        }
    }

    #[test]
    fn a_bare_error_status_is_an_http_error() {
        assert!(matches!(
            parse_response("Play", 500, "Internal Server Error"),
            Err(SoapError::Http(_))
        ));
    }

    #[test]
    fn times_parse_and_format() {
        assert_eq!(parse_time("0:00:00"), Some(0));
        assert_eq!(parse_time("1:02:03.250"), Some(3_723_250));
        assert_eq!(parse_time("00:04:05"), Some(245_000));
        assert_eq!(parse_time("0:00:07.1/2"), Some(7_000));
        assert_eq!(parse_time("NOT_IMPLEMENTED"), None);
        assert_eq!(format_time(3_723_999), "1:02:03");
    }
}
