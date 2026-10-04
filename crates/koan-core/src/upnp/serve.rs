//! The HTTP listener a renderer fetches tracks from and sends events to.
//!
//! It runs only while a renderer is the output. Each track it serves is named
//! by a random token minted for it, which grants that one file and nothing
//! else: library paths and credentials never go on the network. Renderers
//! seek by byte range, and several will not seek at all without it.
//!
//! Hand-rolled over `TcpListener`: three routes, each a single request per
//! connection, from clients that are all simple HTTP/1.1 implementations.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use parking_lot::Mutex;

use super::stream::{self, Pipe};

/// What the listener will serve, by token: a file, or a stream koan is
/// making. Either way `art` is where the cover comes from.
#[derive(Clone)]
pub enum Served {
    File { path: PathBuf, mime: String },
    Stream { pipe: Arc<Pipe>, art: PathBuf },
}

impl Served {
    fn art(&self) -> &std::path::Path {
        match self {
            Self::File { path, .. } => path,
            Self::Stream { art, .. } => art,
        }
    }
}

/// What a `NOTIFY` delivered: the subscription it is for and its body.
pub type EventSink = Box<dyn Fn(&str, String) + Send + Sync>;

/// The listener is open to the whole network while it runs, so what any one
/// client can make it hold is bounded: connections, the request head, and a
/// `NOTIFY` body.
const MAX_CONNECTIONS: usize = 32;
const MAX_HEAD: u64 = 8 * 1024;
const MAX_BODY: usize = 64 * 1024;

struct Shared {
    tracks: Mutex<HashMap<String, Served>>,
    on_event: EventSink,
    stopped: AtomicBool,
    connections: AtomicUsize,
}

pub struct Listener {
    shared: Arc<Shared>,
    port: u16,
}

impl Listener {
    /// Bind an ephemeral port on every interface. Which address a renderer
    /// reaches it on is worked out per renderer: see `local_ip_towards`.
    pub fn start(on_event: EventSink) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("0.0.0.0", 0))?;
        let port = listener.local_addr()?.port();
        let shared = Arc::new(Shared {
            tracks: Mutex::new(HashMap::new()),
            on_event,
            stopped: AtomicBool::new(false),
            connections: AtomicUsize::new(0),
        });
        let accept = shared.clone();
        thread::Builder::new()
            .name("koan-upnp-http".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    if accept.stopped.load(Ordering::Acquire) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    if accept.connections.fetch_add(1, Ordering::AcqRel) >= MAX_CONNECTIONS {
                        accept.connections.fetch_sub(1, Ordering::AcqRel);
                        continue;
                    }
                    let shared = accept.clone();
                    let spawned =
                        thread::Builder::new()
                            .name("koan-upnp-conn".into())
                            .spawn(move || {
                                if let Err(e) = handle(&shared, stream) {
                                    log::debug!("upnp http: {e}");
                                }
                                shared.connections.fetch_sub(1, Ordering::AcqRel);
                            });
                    if spawned.is_err() {
                        accept.connections.fetch_sub(1, Ordering::AcqRel);
                    }
                }
            })?;
        log::info!("upnp: serving on port {port}");
        Ok(Self { shared, port })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Mint a token for `served`. The token alone names the file; the
    /// extension on the path is there for renderers that look at one.
    pub fn add(&self, served: Served) -> String {
        let token = token();
        self.shared.tracks.lock().insert(token.clone(), served);
        token
    }

    pub fn remove(&self, token: &str) {
        self.shared.tracks.lock().remove(token);
    }

    /// Drop every token but those in `keep`.
    pub fn retain(&self, keep: &[&str]) {
        self.shared
            .tracks
            .lock()
            .retain(|token, _| keep.contains(&token.as_str()));
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::Release);
        // Wake the accept loop so it sees the flag.
        let _ = TcpStream::connect_timeout(
            &SocketAddr::from(([127, 0, 0, 1], self.port)),
            Duration::from_millis(200),
        );
    }
}

/// 128 random bits, hex.
fn token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("system randomness");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The address of this machine on the interface that routes to `peer`.
/// Connecting a UDP socket sends nothing; it only asks the routing table.
pub fn local_ip_towards(peer: IpAddr) -> Option<IpAddr> {
    let socket = std::net::UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    socket.connect((peer, 9)).ok()?;
    Some(socket.local_addr().ok()?.ip())
}

pub(crate) struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub(crate) fn read_request(stream: &TcpStream) -> std::io::Result<Request> {
    let mut reader = BufReader::new(stream);
    // The request line and headers together, however slowly they arrive.
    let mut head = (&mut reader).take(MAX_HEAD);
    let mut line = String::new();
    head.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let mut headers = Vec::new();
    loop {
        line.clear();
        if head.read_line(&mut line)? == 0 {
            if head.limit() == 0 {
                return Err(std::io::Error::other("request head too large"));
            }
            break;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
        if headers.len() > 100 {
            return Err(std::io::Error::other("too many headers"));
        }
    }
    let length: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0)
        .min(MAX_BODY);
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok(Request {
        method,
        path,
        headers,
        body,
    })
}

fn handle(shared: &Shared, mut stream: TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let req = read_request(&stream)?;
    let path = req.path.split('?').next().unwrap_or_default();
    let mut segments = path.trim_start_matches('/').splitn(2, '/');
    let route = segments.next().unwrap_or_default();
    let rest = segments.next().unwrap_or_default();
    match (req.method.as_str(), route) {
        ("GET" | "HEAD", "t") => {
            let token = rest.split('.').next().unwrap_or_default();
            let served = shared.tracks.lock().get(token).cloned();
            match served {
                Some(Served::File { path, mime }) => serve_file(&mut stream, &req, &path, &mime),
                Some(Served::Stream { pipe, .. }) => stream::serve(&mut stream, &req, &pipe),
                None => status(&mut stream, 404, "Not Found"),
            }
        }
        ("GET" | "HEAD", "art") => {
            let served = shared.tracks.lock().get(rest).cloned();
            let art = served.and_then(|s| crate::index::metadata::extract_cover_art(s.art()));
            match art {
                Some(bytes) => {
                    let mime = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
                        "image/png"
                    } else {
                        "image/jpeg"
                    };
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        bytes.len()
                    )?;
                    if req.method == "GET" {
                        stream.write_all(&bytes)?;
                    }
                    Ok(())
                }
                None => status(&mut stream, 404, "Not Found"),
            }
        }
        ("NOTIFY", "events") => {
            let sid = req.header("SID").unwrap_or_default().to_string();
            status(&mut stream, 200, "OK")?;
            (shared.on_event)(&sid, String::from_utf8_lossy(&req.body).into_owned());
            Ok(())
        }
        _ => status(&mut stream, 404, "Not Found"),
    }
}

fn status(stream: &mut TcpStream, code: u16, reason: &str) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {code} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
}

/// `bytes=a-b`, `bytes=a-` or `bytes=-n` against a file of `len` bytes, as an
/// inclusive range. `Err` for one that cannot be satisfied.
fn parse_range(header: &str, len: u64) -> Option<Result<(u64, u64), ()>> {
    let spec = header.trim().strip_prefix("bytes=")?;
    // Only the first range; nobody streaming audio asks for several.
    let spec = spec.split(',').next()?.trim();
    let (start, end) = spec.split_once('-')?;
    let range = match (start.trim(), end.trim()) {
        ("", "") => return None,
        ("", n) => {
            let n: u64 = n.parse().ok()?;
            (len.saturating_sub(n), len.saturating_sub(1))
        }
        (a, "") => (a.parse().ok()?, len.saturating_sub(1)),
        (a, b) => (
            a.parse().ok()?,
            b.parse::<u64>().ok()?.min(len.saturating_sub(1)),
        ),
    };
    Some(if range.0 >= len || range.0 > range.1 {
        Err(())
    } else {
        Ok(range)
    })
}

fn serve_file(
    stream: &mut TcpStream,
    req: &Request,
    path: &std::path::Path,
    mime: &str,
) -> std::io::Result<()> {
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return status(stream, 404, "Not Found"),
    };
    let len = file.metadata()?.len();
    let dlna = "transferMode.dlna.org: Streaming\r\ncontentFeatures.dlna.org: DLNA.ORG_OP=01;DLNA.ORG_CI=0;DLNA.ORG_FLAGS=01700000000000000000000000000000\r\n";
    let (start, end, head) = match req.header("Range").map(|r| parse_range(r, len)) {
        Some(Some(Ok((start, end)))) => (
            start,
            end,
            format!("HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{len}\r\n"),
        ),
        Some(Some(Err(()))) => {
            return write!(
                stream,
                "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{len}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
        }
        _ => (0, len.saturating_sub(1), "HTTP/1.1 200 OK\r\n".to_string()),
    };
    let count = if len == 0 { 0 } else { end - start + 1 };
    write!(
        stream,
        "{head}Content-Type: {}\r\nContent-Length: {count}\r\nAccept-Ranges: bytes\r\n{dlna}Connection: close\r\n\r\n",
        mime
    )?;
    if req.method == "HEAD" || count == 0 {
        return Ok(());
    }
    file.seek(SeekFrom::Start(start))?;
    std::io::copy(&mut file.take(count), stream)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(port: u16, request: &str) -> Vec<u8> {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut out = Vec::new();
        stream.read_to_end(&mut out).unwrap();
        out
    }

    fn split(response: &[u8]) -> (String, Vec<u8>) {
        let at = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        (
            String::from_utf8_lossy(&response[..at]).into_owned(),
            response[at + 4..].to_vec(),
        )
    }

    #[test]
    fn ranges_parse() {
        assert_eq!(parse_range("bytes=0-", 10), Some(Ok((0, 9))));
        assert_eq!(parse_range("bytes=2-4", 10), Some(Ok((2, 4))));
        assert_eq!(parse_range("bytes=-3", 10), Some(Ok((7, 9))));
        assert_eq!(parse_range("bytes=5-100", 10), Some(Ok((5, 9))));
        assert_eq!(parse_range("bytes=10-", 10), Some(Err(())));
        assert_eq!(parse_range("items=0-1", 10), None);
    }

    #[test]
    fn a_token_serves_its_file_whole_and_by_range_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.flac");
        std::fs::write(&path, b"0123456789").unwrap();
        let listener = Listener::start(Box::new(|_, _| {})).unwrap();
        let token = listener.add(Served::File {
            path,
            mime: "audio/flac".into(),
        });
        let port = listener.port();

        let (head, body) = split(&get(
            port,
            &format!("GET /t/{token}.flac HTTP/1.1\r\nHost: x\r\n\r\n"),
        ));
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert!(head.contains("Content-Type: audio/flac"));
        assert!(head.contains("Accept-Ranges: bytes"));
        assert_eq!(body, b"0123456789");

        let (head, body) = split(&get(
            port,
            &format!("GET /t/{token}.flac HTTP/1.1\r\nRange: bytes=3-5\r\n\r\n"),
        ));
        assert!(head.starts_with("HTTP/1.1 206"), "{head}");
        assert!(head.contains("Content-Range: bytes 3-5/10"));
        assert_eq!(body, b"345");

        let (head, body) = split(&get(port, &format!("HEAD /t/{token} HTTP/1.1\r\n\r\n")));
        assert!(head.contains("Content-Length: 10"));
        assert!(body.is_empty());

        let (head, _) = split(&get(port, "GET /t/0000 HTTP/1.1\r\n\r\n"));
        assert!(head.starts_with("HTTP/1.1 404"));

        listener.retain(&[]);
        let (head, _) = split(&get(port, &format!("GET /t/{token}.flac HTTP/1.1\r\n\r\n")));
        assert!(head.starts_with("HTTP/1.1 404"));
    }

    #[test]
    fn notify_reaches_the_sink() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let listener = Listener::start(Box::new(move |sid, body| {
            tx.send((sid.to_string(), body)).unwrap();
        }))
        .unwrap();
        let body = "<e:propertyset/>";
        let (head, _) = split(&get(
            listener.port(),
            &format!(
                "NOTIFY /events/x HTTP/1.1\r\nSID: uuid:sub-1\r\nNT: upnp:event\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        ));
        assert!(head.starts_with("HTTP/1.1 200"));
        assert_eq!(
            rx.recv().unwrap(),
            ("uuid:sub-1".to_string(), body.to_string())
        );
    }
}
