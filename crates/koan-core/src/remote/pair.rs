//! Pairing: signing in a device that has no keyboard to type a password with.
//!
//! The device knows only the server's address. It opens a WebSocket at
//! `/rest/koanPair` and is given a pairing: an unguessable id for a link, and
//! a short code to read off the screen. Someone signed in elsewhere — the
//! phone, the Mac, the server's web page — approves it, and the server makes
//! an API key on their account and sends it down the waiting socket. Nothing
//! asks the server whether it has happened yet: the answer arrives when it
//! does. A koan extension, `koanPair`.
//!
//! The link carries the server and the id in the fragment of a koan.rocks
//! address, as invites do, so the site serving the page never sees them.

use std::net::{Shutdown, TcpStream};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tungstenite::stream::MaybeTlsStream;
use url::{Url, form_urlencoded};

use crate::config::Config;
use crate::helpers::SignInError;
use crate::remote::client::SubsonicError;

pub const PAIR_PAGE: &str = "https://koan.rocks/pair/";

/// The server pings a waiting device every half minute. Three missed mean
/// the connection is gone.
const SILENCE: Duration = Duration::from_secs(90);

/// What the server sends down a pairing socket: `Pending` once it is open,
/// then one outcome before it closes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PairMessage {
    #[serde(rename_all = "camelCase")]
    Pending {
        id: String,
        /// `XXXX-XXXX`, for someone to type.
        code: String,
        /// Seconds until the pairing lapses.
        expires_in: u64,
    },
    #[serde(rename_all = "camelCase")]
    Approved {
        username: String,
        api_key: String,
    },
    Declined,
    Expired,
}

#[derive(Debug, thiserror::Error)]
pub enum PairError {
    #[error("not an http(s) server address: {0}")]
    BadUrl(String),
    #[error("could not reach the server: {0}")]
    Connect(String),
    /// The server answered, and has no pairing to offer: not koan, or a koan
    /// from before pairing.
    #[error("this server does not pair devices")]
    Unsupported,
    #[error("too many devices are waiting to pair from this network; try again in a few minutes")]
    TooMany,
    #[error("the server answered with something unexpected")]
    BadResponse,
    #[error("the sign-in was declined")]
    Declined,
    #[error("the code expired before anyone approved it")]
    Expired,
    #[error("the connection to the server closed")]
    Closed,
    #[error("no server is signed in")]
    NotSignedIn,
    #[error(transparent)]
    Remote(#[from] SubsonicError),
    #[error(transparent)]
    SignIn(#[from] SignInError),
}

type Socket = tungstenite::WebSocket<MaybeTlsStream<TcpStream>>;

/// A pairing the server is holding open for this device.
pub struct Pending {
    pub id: String,
    pub code: String,
    /// The universal link that opens the approval in the app, or the server's
    /// own page for someone without it.
    pub link: String,
    server: String,
    socket: Socket,
}

/// Ends a wait from another thread.
pub struct Cancel(TcpStream);

impl Cancel {
    pub fn cancel(&self) {
        let _ = self.0.shutdown(Shutdown::Both);
    }
}

/// Ask the server at `url` to pair this device, which approvers will see
/// called `device`.
pub fn start(url: &str, device: &str) -> Result<Pending, PairError> {
    let server = url.trim().trim_end_matches('/').to_owned();
    let socket_url = pair_url(&server, device)?;
    let (mut socket, _) = tungstenite::connect(socket_url.as_str()).map_err(|e| match e {
        // Any answer but an upgrade is a server without pairing, except a
        // koan turning away a busy address and a proxy whose server is down.
        tungstenite::Error::Http(ref response) => match response.status().as_u16() {
            429 => PairError::TooMany,
            502..=504 => PairError::Connect(e.to_string()),
            _ => PairError::Unsupported,
        },
        e => PairError::Connect(e.to_string()),
    })?;
    if let Some(tcp) = tcp(&socket) {
        let _ = tcp.set_read_timeout(Some(SILENCE));
    }
    match read(&mut socket)? {
        PairMessage::Pending { id, code, .. } => Ok(Pending {
            link: link(&server, &id),
            id,
            code,
            server,
            socket,
        }),
        _ => Err(PairError::BadResponse),
    }
}

impl Pending {
    /// What stops `wait`, for whoever gives up on it.
    pub fn canceller(&self) -> Option<Cancel> {
        tcp(&self.socket)?.try_clone().ok().map(Cancel)
    }

    /// Block until the pairing is approved, declined or lapses. Approved, the
    /// key it brings is stored as an invite's is, and the app is signed in.
    pub fn wait(mut self) -> Result<(), PairError> {
        let outcome = read(&mut self.socket);
        let _ = self.socket.close(None);
        match outcome? {
            PairMessage::Approved { username, api_key } => {
                crate::helpers::adopt_api_key(&self.server, &username, &api_key)?;
                Ok(())
            }
            PairMessage::Declined => Err(PairError::Declined),
            PairMessage::Expired => Err(PairError::Expired),
            PairMessage::Pending { .. } => Err(PairError::BadResponse),
        }
    }
}

/// The next message, answering pings while it waits.
fn read(socket: &mut Socket) -> Result<PairMessage, PairError> {
    loop {
        match socket.read() {
            Ok(tungstenite::Message::Text(text)) => {
                return serde_json::from_str(&text).map_err(|_| PairError::BadResponse);
            }
            Ok(tungstenite::Message::Close(_)) => return Err(PairError::Closed),
            Ok(_) => {}
            Err(_) => return Err(PairError::Closed),
        }
    }
}

fn tcp(socket: &Socket) -> Option<&TcpStream> {
    match socket.get_ref() {
        MaybeTlsStream::Plain(s) => Some(s),
        MaybeTlsStream::Rustls(s) => Some(s.get_ref()),
        _ => None,
    }
}

fn pair_url(server: &str, device: &str) -> Result<Url, PairError> {
    let bad = || PairError::BadUrl(server.to_owned());
    let mut url = Url::parse(&format!("{server}/rest/koanPair")).map_err(|_| bad())?;
    let scheme = match url.scheme() {
        "https" => "wss",
        "http" => "ws",
        _ => return Err(bad()),
    };
    url.set_scheme(scheme).map_err(|()| bad())?;
    url.query_pairs_mut().append_pair("name", device);
    Ok(url)
}

/// The link that approves pairing `id` on `server`.
pub fn link(server: &str, id: &str) -> String {
    let fragment = form_urlencoded::Serializer::new(String::new())
        .append_pair("s", server)
        .append_pair("p", id)
        .finish();
    format!("{PAIR_PAGE}#{fragment}")
}

/// A pairing link's server and pairing id. `None` for anything else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairLink {
    pub server: String,
    pub id: String,
}

impl PairLink {
    pub fn parse(link: &str) -> Option<Self> {
        let url = Url::parse(link.trim()).ok()?;
        if url.scheme() != "https"
            || url.host_str() != Some("koan.rocks")
            || url.path().trim_end_matches('/') != "/pair"
        {
            return None;
        }
        let (mut server, mut id) = (None, None);
        for (k, v) in form_urlencoded::parse(url.fragment()?.as_bytes()) {
            let v = Some(v.trim().to_owned()).filter(|v| !v.is_empty());
            match &*k {
                "s" => server = v,
                "p" => id = v,
                _ => {}
            }
        }
        let server = server?.trim_end_matches('/').to_owned();
        if !matches!(Url::parse(&server).ok()?.scheme(), "http" | "https") {
            return None;
        }
        Some(Self { server, id: id? })
    }
}

fn client() -> Result<std::sync::Arc<crate::remote::client::SubsonicClient>, PairError> {
    crate::helpers::subsonic_client(&Config::load().unwrap_or_default())
        .ok_or(PairError::NotSignedIn)
}

/// The device waiting on `pair` (an id or a code) on the signed-in server,
/// and where it asked from.
pub fn info(pair: &str) -> Result<crate::remote::client::KoanPair, PairError> {
    Ok(client()?.koan_pair_info(pair)?)
}

/// Sign the device waiting on `pair` in as the signed-in account. Answers
/// with the device's name.
pub fn approve(pair: &str) -> Result<String, PairError> {
    Ok(client()?.koan_pair_approve(pair, false)?)
}

/// Turn the device waiting on `pair` away.
pub fn decline(pair: &str) -> Result<String, PairError> {
    Ok(client()?.koan_pair_approve(pair, true)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_reads_back() {
        let made = link("https://music.example.com:8443/koan", "abc_-123");
        assert!(made.starts_with("https://koan.rocks/pair/#s=https%3A%2F%2Fmusic"));
        assert_eq!(
            PairLink::parse(&made),
            Some(PairLink {
                server: "https://music.example.com:8443/koan".into(),
                id: "abc_-123".into(),
            })
        );
        assert!(PairLink::parse("https://koan.rocks/pair#s=https%3A%2F%2Fa.example&p=x").is_some());
    }

    #[test]
    fn other_links_are_not_pairings() {
        for link in [
            "https://koan.rocks/join/#s=https%3A%2F%2Fa.example&p=x",
            "https://evil.example/pair/#s=https%3A%2F%2Fa.example&p=x",
            "https://koan.rocks/pair/#s=javascript%3Aalert(1)&p=x",
            "https://koan.rocks/pair/#s=https%3A%2F%2Fa.example",
            "https://koan.rocks/pair/#p=x",
        ] {
            assert_eq!(PairLink::parse(link), None, "{link}");
        }
    }

    #[test]
    fn the_socket_address_follows_the_scheme() {
        let url = pair_url("https://music.example.com/koan", "Living room TV").unwrap();
        assert_eq!(
            url.as_str(),
            "wss://music.example.com/koan/rest/koanPair?name=Living+room+TV"
        );
        let url = pair_url("http://10.0.0.2:4533", "tv").unwrap();
        assert_eq!(url.scheme(), "ws");
        assert!(pair_url("ftp://x", "tv").is_err());
    }

    #[test]
    fn info_carries_where_the_request_came_from() {
        let info: crate::remote::client::KoanPair =
            serde_json::from_str(r#"{"device":"Den TV","from":"192.168.1.20","local":true}"#)
                .unwrap();
        assert_eq!((info.from.as_str(), info.local), ("192.168.1.20", true));
    }

    #[test]
    fn messages_are_the_wire_format() {
        let pending: PairMessage = serde_json::from_str(
            r#"{"type":"pending","id":"i","code":"ABCD-EFGH","expiresIn":600}"#,
        )
        .unwrap();
        assert_eq!(
            pending,
            PairMessage::Pending {
                id: "i".into(),
                code: "ABCD-EFGH".into(),
                expires_in: 600
            }
        );
        let approved = PairMessage::Approved {
            username: "alice".into(),
            api_key: "k".into(),
        };
        assert_eq!(
            serde_json::to_string(&approved).unwrap(),
            r#"{"type":"approved","username":"alice","apiKey":"k"}"#
        );
        assert_eq!(
            serde_json::to_string(&PairMessage::Expired).unwrap(),
            r#"{"type":"expired"}"#
        );
    }
}
