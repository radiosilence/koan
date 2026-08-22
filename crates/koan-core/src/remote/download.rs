//! Streaming file downloads: temp file, progress reporting, atomic rename, retries.
//!
//! Every remote byte koan writes to disk goes through here. `dest` only ever
//! appears once the transfer completed, so a partially-written file can never
//! be mistaken for a cached track.

use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

use thiserror::Error;

/// Longest the TCP connect + TLS handshake may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Longest a single body read may block before the transfer counts as stalled.
///
/// `reqwest`'s blocking client re-applies its request timeout to each `Read`
/// of a streamed response, so this bounds *stalls*, not total transfer time —
/// a large file on a slow link keeps going as long as bytes keep arriving.
const STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// Total deadline for JSON API calls, whose bodies are small and read in one go.
pub const API_TIMEOUT: Duration = Duration::from_secs(30);

/// Attempts a download gets before giving up.
pub const DEFAULT_ATTEMPTS: u32 = 3;

/// Base backoff between attempts; doubles each retry.
const BACKOFF_BASE: Duration = Duration::from_millis(500);

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("incomplete download: got {got} of {expected} bytes")]
    Incomplete { got: u64, expected: u64 },
    #[error("server returned {0}")]
    Status(reqwest::StatusCode),
}

impl DownloadError {
    /// Whether another attempt could plausibly succeed: transport-level
    /// failures, truncated bodies, and server-side/rate-limit statuses.
    pub fn is_retryable(&self) -> bool {
        match self {
            DownloadError::Http(e) => e.is_timeout() || e.is_connect() || e.is_request(),
            DownloadError::Io(_) | DownloadError::Incomplete { .. } => true,
            DownloadError::Status(s) => {
                s.is_server_error() || *s == reqwest::StatusCode::TOO_MANY_REQUESTS
            }
        }
    }
}

/// HTTP client for streaming large bodies — bounded connect, bounded stalls,
/// no total deadline on the transfer.
pub fn download_client() -> reqwest::Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(STALL_TIMEOUT)
        .build()
}

/// HTTP client for small JSON API calls, where a total request deadline is correct.
pub fn api_client() -> reqwest::Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(API_TIMEOUT)
        .build()
}

/// Download to `dest`, retrying transient failures with exponential backoff.
///
/// `request` is invoked once per attempt so per-request state (Subsonic auth
/// salts, for one) is regenerated rather than replayed. `on_progress` receives
/// `(bytes_this_attempt, total)` where `total` is 0 if the server sent no
/// Content-Length; it restarts from zero when an attempt is retried.
///
/// Returns the number of bytes written. `dest` is left untouched on failure.
pub fn download_with_retries(
    dest: &Path,
    attempts: u32,
    request: impl Fn() -> reqwest::blocking::RequestBuilder,
    on_progress: impl Fn(u64, u64),
) -> Result<u64, DownloadError> {
    let attempts = attempts.max(1);
    let mut last_err = None;

    for attempt in 0..attempts {
        if attempt > 0 {
            let backoff = BACKOFF_BASE * 2u32.pow(attempt - 1);
            log::warn!(
                "download of {} failed ({}), retrying in {:?} ({}/{})",
                dest.display(),
                last_err
                    .as_ref()
                    .map(|e: &DownloadError| e.to_string())
                    .unwrap_or_default(),
                backoff,
                attempt + 1,
                attempts
            );
            std::thread::sleep(backoff);
        }

        match attempt_download(dest, &request, &on_progress) {
            Ok(bytes) => return Ok(bytes),
            Err(e) if e.is_retryable() => last_err = Some(e),
            Err(e) => return Err(e),
        }
    }

    Err(last_err.expect("loop runs at least once and only continues on error"))
}

fn attempt_download(
    dest: &Path,
    request: &impl Fn() -> reqwest::blocking::RequestBuilder,
    on_progress: &impl Fn(u64, u64),
) -> Result<u64, DownloadError> {
    let resp = request().send()?;
    let status = resp.status();
    if !status.is_success() {
        return Err(DownloadError::Status(status));
    }
    stream_to_file(resp, dest, on_progress)
}

/// Stream a response body into `dest` via a `.part` sibling, renaming only once
/// the transfer completes. A read error or a body shorter than the advertised
/// Content-Length removes the temp file and errors.
fn stream_to_file(
    mut resp: reqwest::blocking::Response,
    dest: &Path,
    on_progress: &impl Fn(u64, u64),
) -> Result<u64, DownloadError> {
    let total = resp.content_length().unwrap_or(0);

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let tmp = part_path(dest);
    let mut file = std::fs::File::create(&tmp)?;
    let mut downloaded: u64 = 0;
    let mut buf = [0u8; 64 * 1024];

    let result = loop {
        match resp.read(&mut buf) {
            Ok(0) => break Ok(()),
            Ok(n) => {
                if let Err(e) = file.write_all(&buf[..n]) {
                    break Err(DownloadError::Io(e));
                }
                downloaded += n as u64;
                on_progress(downloaded, total);
            }
            Err(e) => break Err(DownloadError::Io(e)),
        }
    };

    let flushed = file.flush();
    drop(file);

    let outcome = result
        .and_then(|()| flushed.map_err(DownloadError::Io))
        .and_then(|()| {
            if total > 0 && downloaded != total {
                Err(DownloadError::Incomplete {
                    got: downloaded,
                    expected: total,
                })
            } else {
                Ok(())
            }
        });

    if let Err(e) = outcome {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }

    std::fs::rename(&tmp, dest)?;
    Ok(downloaded)
}

/// The in-progress sibling of `dest`. Appends `.part` rather than replacing the
/// extension, so `Song.flac` and `Song.mp3` never collide on one temp file.
pub fn part_path(dest: &Path) -> std::path::PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

/// Strip a `.part` suffix, yielding the final path a download will land at.
/// Returns `path` unchanged when it isn't a temp file.
pub fn strip_part_suffix(path: &Path) -> std::path::PathBuf {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return path.to_path_buf();
    };
    match name.strip_suffix(".part") {
        Some(stripped) => path.with_file_name(stripped),
        None => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// How a stub server answers one request.
    #[derive(Clone)]
    enum Reply {
        /// Content-Length header, then that many bytes.
        Complete(Vec<u8>),
        /// Content-Length claims `claimed` bytes but only `body` is sent, then close.
        Truncated {
            claimed: usize,
            body: Vec<u8>,
        },
        /// Chunked with no Content-Length, cut off mid-stream — what Navidrome
        /// does for transcoded streams when the connection drops.
        ChunkedTruncated(Vec<u8>),
        ServerError,
    }

    /// Single-threaded stub HTTP server. Serves `replies` in order, repeating
    /// the last one forever. Shuts down when the returned handle is dropped.
    struct StubServer {
        addr: std::net::SocketAddr,
        hits: Arc<AtomicUsize>,
        shutdown: Arc<std::sync::atomic::AtomicBool>,
    }

    impl StubServer {
        fn start(replies: Vec<Reply>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let addr = listener.local_addr().unwrap();
            let hits = Arc::new(AtomicUsize::new(0));
            let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));

            let hits_bg = hits.clone();
            let shutdown_bg = shutdown.clone();
            std::thread::spawn(move || {
                while !shutdown_bg.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            // BSD sockets inherit O_NONBLOCK from the listener.
                            let _ = stream.set_nonblocking(false);
                            let n = hits_bg.fetch_add(1, Ordering::SeqCst);
                            let reply = replies[n.min(replies.len() - 1)].clone();
                            serve_one(stream, reply);
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            });

            Self {
                addr,
                hits,
                shutdown,
            }
        }

        fn url(&self) -> String {
            format!("http://{}/file", self.addr)
        }

        fn hits(&self) -> usize {
            self.hits.load(Ordering::SeqCst)
        }
    }

    impl Drop for StubServer {
        fn drop(&mut self) {
            self.shutdown.store(true, Ordering::Relaxed);
        }
    }

    fn serve_one(mut stream: TcpStream, reply: Reply) {
        // Drain the request headers so the client isn't left writing into a
        // closed socket before it can read the response.
        let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            if line == "\r\n" || line == "\n" {
                break;
            }
            line.clear();
        }

        match reply {
            Reply::Complete(body) => {
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
            }
            Reply::Truncated { claimed, body } => {
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                    claimed
                );
                let _ = stream.write_all(&body);
            }
            Reply::ChunkedTruncated(body) => {
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n"
                );
                let _ = write!(stream, "{:x}\r\n", body.len());
                let _ = stream.write_all(&body);
                let _ = stream.write_all(b"\r\n");
                // No terminating zero-length chunk — the stream just stops.
            }
            Reply::ServerError => {
                let _ = write!(stream, "HTTP/1.1 500 Internal Server Error\r\n\r\n");
            }
        }
        let _ = stream.flush();
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }

    fn tmp_dest(dir: &tempfile::TempDir) -> std::path::PathBuf {
        dir.path().join("nested").join("track.flac")
    }

    #[test]
    fn complete_download_lands_at_dest() {
        let body = vec![7u8; 200_000];
        let server = StubServer::start(vec![Reply::Complete(body.clone())]);
        let dir = tempfile::tempdir().unwrap();
        let dest = tmp_dest(&dir);
        let client = download_client().unwrap();

        let written =
            download_with_retries(&dest, 1, || client.get(server.url()), |_, _| {}).unwrap();

        assert_eq!(written, body.len() as u64);
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert!(!part_path(&dest).exists(), "temp file should be cleaned up");
    }

    #[test]
    fn truncated_body_errors_and_leaves_no_file() {
        let server = StubServer::start(vec![Reply::Truncated {
            claimed: 100_000,
            body: vec![1u8; 4_096],
        }]);
        let dir = tempfile::tempdir().unwrap();
        let dest = tmp_dest(&dir);
        let client = download_client().unwrap();

        let err = download_with_retries(&dest, 1, || client.get(server.url()), |_, _| {})
            .expect_err("a short body must not succeed");

        assert!(
            matches!(err, DownloadError::Incomplete { .. } | DownloadError::Io(_)),
            "unexpected error: {err}"
        );
        assert!(!dest.exists(), "dest must not hold a truncated file");
        assert!(!part_path(&dest).exists(), "temp file must be removed");
    }

    #[test]
    fn missing_content_length_truncation_errors_rather_than_completing() {
        // No Content-Length at all — the only signal is the stream ending
        // mid-message, which must not be read as a finished download.
        let server = StubServer::start(vec![Reply::ChunkedTruncated(vec![9u8; 8_192])]);
        let dir = tempfile::tempdir().unwrap();
        let dest = tmp_dest(&dir);
        let client = download_client().unwrap();

        let err = download_with_retries(&dest, 1, || client.get(server.url()), |_, _| {})
            .expect_err("a cut-off chunked body must not succeed");

        assert!(matches!(err, DownloadError::Io(_)), "unexpected: {err}");
        assert!(!dest.exists(), "dest must not hold a truncated file");
        assert!(!part_path(&dest).exists(), "temp file must be removed");
    }

    #[test]
    fn retries_transient_failure_then_succeeds() {
        let body = vec![3u8; 50_000];
        let server = StubServer::start(vec![
            Reply::ServerError,
            Reply::Truncated {
                claimed: 50_000,
                body: vec![3u8; 10],
            },
            Reply::Complete(body.clone()),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let dest = tmp_dest(&dir);
        let client = download_client().unwrap();

        let written =
            download_with_retries(&dest, 3, || client.get(server.url()), |_, _| {}).unwrap();

        assert_eq!(written, body.len() as u64);
        assert_eq!(server.hits(), 3, "should have used all three attempts");
        assert_eq!(std::fs::read(&dest).unwrap(), body);
    }

    #[test]
    fn progress_reports_total_when_content_length_present() {
        let body = vec![0u8; 300_000];
        let server = StubServer::start(vec![Reply::Complete(body.clone())]);
        let dir = tempfile::tempdir().unwrap();
        let dest = tmp_dest(&dir);
        let client = download_client().unwrap();

        let seen = std::sync::Mutex::new(Vec::new());
        download_with_retries(
            &dest,
            1,
            || client.get(server.url()),
            |d, t| {
                seen.lock().unwrap().push((d, t));
            },
        )
        .unwrap();

        let seen = seen.into_inner().unwrap();
        assert!(!seen.is_empty(), "progress should be reported");
        assert!(seen.iter().all(|(_, t)| *t == body.len() as u64));
        assert_eq!(seen.last().unwrap().0, body.len() as u64);
    }

    #[test]
    fn part_path_appends_rather_than_replacing_extension() {
        let flac = part_path(Path::new("/tmp/Song.flac"));
        let mp3 = part_path(Path::new("/tmp/Song.mp3"));
        assert_eq!(flac, Path::new("/tmp/Song.flac.part"));
        assert_ne!(flac, mp3, "different codecs must not share a temp file");
    }

    #[test]
    fn strip_part_suffix_round_trips() {
        let dest = Path::new("/tmp/a/Song.flac");
        assert_eq!(strip_part_suffix(&part_path(dest)), dest);
        assert_eq!(strip_part_suffix(dest), dest);
    }
}
