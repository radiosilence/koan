//! Streaming file downloads: temp file, progress reporting, atomic rename, retries.
//!
//! Every remote byte koan writes to disk goes through here. `dest` only ever
//! appears once the transfer completed, so a partially-written file can never
//! be mistaken for a cached track.

use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
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

/// Names the device class, so a server that tells players apart by user agent
/// (Navidrome does) sees a Mac and a phone as two players, not one.
#[cfg(target_os = "macos")]
const USER_AGENT: &str = concat!("koan/", env!("CARGO_PKG_VERSION"), " (Macintosh)");
#[cfg(target_os = "ios")]
const USER_AGENT: &str = concat!("koan/", env!("CARGO_PKG_VERSION"), " (iOS)");
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
const USER_AGENT: &str = concat!("koan/", env!("CARGO_PKG_VERSION"), " (Linux)");

/// Attempts a download gets before giving up.
pub const DEFAULT_ATTEMPTS: u32 = 3;

/// Base backoff between attempts; doubles each retry.
const BACKOFF_BASE: Duration = Duration::from_millis(500);

/// Waits between tries at a server that is not answering, by how many times in
/// a row it has not. The last repeats for as long as the outage lasts.
const OUTAGE_BACKOFF: [Duration; 3] = [
    Duration::from_secs(5),
    Duration::from_secs(15),
    Duration::from_secs(60),
];

/// Longest `Retry-After` koan honours. A server asking for more is asked again
/// at this interval instead, so a phone does not sit silent for an hour on the
/// word of a misconfigured proxy.
const RETRY_AFTER_CAP: Duration = Duration::from_secs(600);

/// How often a download waiting out an outage checks whether it is still wanted.
const CANCEL_POLL: Duration = Duration::from_secs(1);

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("incomplete download: got {got} of {expected} bytes")]
    Incomplete { got: u64, expected: u64 },
    #[error("server returned {status}")]
    Status {
        status: reqwest::StatusCode,
        /// The server's `Retry-After`, when it sent one in seconds.
        retry_after: Option<Duration>,
    },
    #[error("request could not be built: {0}")]
    Request(String),
    #[error("no longer wanted")]
    Cancelled,
}

impl DownloadError {
    /// Whether another attempt could plausibly succeed: transport-level
    /// failures, truncated bodies, and server-side/rate-limit statuses.
    pub fn is_retryable(&self) -> bool {
        match self {
            DownloadError::Http(e) => e.is_timeout() || e.is_connect() || e.is_request(),
            DownloadError::Io(_) | DownloadError::Incomplete { .. } => true,
            DownloadError::Status { status, .. } => {
                status.is_server_error() || *status == reqwest::StatusCode::TOO_MANY_REQUESTS
            }
            DownloadError::Request(_) | DownloadError::Cancelled => false,
        }
    }

    /// Whether this says the server is not answering at all, rather than that
    /// this track cannot be had. Every other download would get the same
    /// answer, so it is waited out instead of counted against the track.
    ///
    /// A 500, a 404, a body cut short or an error document are about the track.
    pub fn is_unavailable(&self) -> bool {
        use reqwest::StatusCode;
        match self {
            // Refused, no route, no DNS, and a connect that timed out.
            DownloadError::Http(e) => e.is_connect(),
            DownloadError::Status { status, .. } => matches!(
                *status,
                StatusCode::SERVICE_UNAVAILABLE
                    | StatusCode::TOO_MANY_REQUESTS
                    | StatusCode::BAD_GATEWAY
                    | StatusCode::GATEWAY_TIMEOUT
            ),
            _ => false,
        }
    }

    fn retry_after(&self) -> Option<Duration> {
        match self {
            DownloadError::Status { retry_after, .. } => *retry_after,
            _ => None,
        }
    }
}

/// How long to wait before trying a server that has not answered `failures`
/// times in a row (counting from 1). The server's own `Retry-After` wins.
pub fn outage_backoff(failures: u32, retry_after: Option<Duration>) -> Duration {
    if let Some(wait) = retry_after {
        return wait.min(RETRY_AFTER_CAP);
    }
    let step = (failures.max(1) - 1) as usize;
    OUTAGE_BACKOFF[step.min(OUTAGE_BACKOFF.len() - 1)]
}

/// `Retry-After` in delta-seconds. The HTTP-date form is left to the schedule.
fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
        .map(Duration::from_secs)
}

/// Whether a server is answering, shared by every download against it.
///
/// One transfer finding it down makes the rest wait with it, and while it is
/// down only one of them at a time asks again. Without this each download
/// learnt of the outage for itself, gave up, and the player moved on to the
/// next track to do the same.
#[derive(Default)]
pub struct Outage {
    state: Mutex<OutageState>,
    changed: Condvar,
}

#[derive(Default)]
struct OutageState {
    /// Unanswered tries in a row. Zero while the server is up.
    failures: u32,
    /// When the next try may go. `None` while the server is up.
    retry_at: Option<Instant>,
}

impl Outage {
    pub fn is_down(&self) -> bool {
        self.state.lock().retry_at.is_some()
    }

    /// Block while the server is down and nobody is due to try it. Returns
    /// immediately when it is up, or once a try is due.
    pub fn hold(&self) {
        let mut s = self.state.lock();
        while let Some(at) = s.retry_at {
            let now = Instant::now();
            if now >= at {
                break;
            }
            self.changed.wait_for(&mut s, at - now);
        }
    }

    /// Wait until this download may try the server: at once when it is up,
    /// otherwise when the next try is due and no other download has taken it.
    /// Returns `false` if `cancelled` says the download stopped being wanted.
    fn admit(&self, cancelled: &dyn Fn() -> bool) -> bool {
        let mut s = self.state.lock();
        loop {
            // Asked outside the lock: it reads the player's state.
            if parking_lot::MutexGuard::unlocked(&mut s, cancelled) {
                return false;
            }
            let Some(at) = s.retry_at else {
                return true;
            };
            let now = Instant::now();
            if now >= at {
                // This one tries; the rest wait for its answer. Pushed out
                // rather than flagged, so a try that never reports back
                // (it panicked, say) holds the others up for one step only.
                s.retry_at = Some(now + outage_backoff(s.failures, None));
                return true;
            }
            self.changed.wait_for(&mut s, (at - now).min(CANCEL_POLL));
        }
    }

    /// The server did not answer. Returns how long until it is tried again.
    fn down(&self, retry_after: Option<Duration>) -> Duration {
        let mut s = self.state.lock();
        s.failures += 1;
        let wait = outage_backoff(s.failures, retry_after);
        s.retry_at = Some(Instant::now() + wait);
        drop(s);
        // Those waiting for this answer take the new time from here.
        self.changed.notify_all();
        wait
    }

    /// The server answered, whatever it said.
    fn up(&self) {
        let mut s = self.state.lock();
        if s.retry_at.is_none() {
            return;
        }
        log::info!(
            "remote server answering again after {} failed tries",
            s.failures
        );
        *s = OutageState::default();
        drop(s);
        self.changed.notify_all();
    }
}

/// Wait out a server that is not answering rather than fail against it.
pub struct Patience<'a> {
    pub outage: &'a Outage,
    /// Asked while waiting; `true` ends the wait with `DownloadError::Cancelled`.
    pub cancelled: &'a dyn Fn() -> bool,
}

/// HTTP client for streaming large bodies — bounded connect, bounded stalls,
/// no total deadline on the transfer.
pub fn download_client() -> reqwest::Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(STALL_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
}

/// HTTP client for small JSON API calls, where a total request deadline is correct.
pub fn api_client() -> reqwest::Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(API_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
}

/// Download to `dest`, retrying transient failures with exponential backoff.
///
/// `request` is invoked once per attempt so per-request state (Subsonic auth
/// salts, for one) is regenerated rather than replayed; a request that cannot
/// be built is fatal, not retried. `on_progress` receives
/// `(bytes_this_attempt, total)` where `total` is 0 if the server sent no
/// Content-Length; it restarts from zero when an attempt is retried.
///
/// With `patience`, a server that is not answering (`is_unavailable`) is waited
/// out on its `Outage` for as long as it takes, and those tries do not count
/// against `attempts`. Without it they are retried like any transient failure.
///
/// Returns the number of bytes written. `dest` is left untouched on failure.
pub fn download_with_retries(
    dest: &Path,
    attempts: u32,
    patience: Option<Patience<'_>>,
    request: impl Fn() -> Result<reqwest::blocking::RequestBuilder, DownloadError>,
    on_progress: impl Fn(u64, u64),
) -> Result<u64, DownloadError> {
    let attempts = attempts.max(1);
    let mut failures = 0;

    let err = loop {
        if let Some(p) = &patience
            && !p.outage.admit(p.cancelled)
        {
            break DownloadError::Cancelled;
        }

        let e = match attempt_download(dest, &request, &on_progress) {
            Ok(bytes) => {
                if let Some(p) = &patience {
                    p.outage.up();
                }
                return Ok(bytes);
            }
            Err(e) => e,
        };

        if let Some(p) = &patience {
            if e.is_unavailable() {
                let wait = p.outage.down(e.retry_after());
                log::warn!(
                    "download of {}: server unavailable ({}), trying again in {:?}",
                    dest.display(),
                    e,
                    wait
                );
                continue;
            }
            p.outage.up();
        }

        failures += 1;
        if !e.is_retryable() || failures >= attempts {
            break e;
        }
        let backoff = BACKOFF_BASE * 2u32.pow(failures - 1);
        log::warn!(
            "download of {} failed ({}), retrying in {:?} ({}/{})",
            dest.display(),
            e,
            backoff,
            failures + 1,
            attempts
        );
        std::thread::sleep(backoff);
    };

    // Only once the download has given up. Between attempts the `.part` stays,
    // and the next one truncates it in place: a stream already reading it holds
    // that inode, and picks up again as the retry rewrites the same bytes.
    let _ = std::fs::remove_file(part_path(dest));
    Err(err)
}

fn attempt_download(
    dest: &Path,
    request: &impl Fn() -> Result<reqwest::blocking::RequestBuilder, DownloadError>,
    on_progress: &impl Fn(u64, u64),
) -> Result<u64, DownloadError> {
    let resp = request()?.send()?;
    let status = resp.status();
    // Status first: a 503 with a JSON body is transient and worth retrying.
    if !status.is_success() {
        return Err(DownloadError::Status {
            status,
            retry_after: parse_retry_after(resp.headers()),
        });
    }
    // Subsonic reports failure with HTTP 200 and a JSON or XML error body, so a
    // success status proves nothing on a binary endpoint. Without this, an error
    // response gets written to disk and cached as if it were audio — it then
    // reports Ready and fails to decode forever.
    if resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.contains("json") || ct.contains("xml"))
    {
        return Err(DownloadError::Request(
            "server returned an error document where audio was expected".into(),
        ));
    }
    stream_to_file(resp, dest, on_progress)
}

/// Stream a response body into `dest` via a `.part` sibling, renaming only once
/// the transfer completes. A read error or a body shorter than the advertised
/// Content-Length errors, leaving the temp file for the caller to retry into.
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

    outcome?;
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
        /// 503, with a `Retry-After` in seconds when given.
        Unavailable(Option<u64>),
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
                    "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
            }
            Reply::Truncated { claimed, body } => {
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
                    claimed
                );
                let _ = stream.write_all(&body);
            }
            Reply::ChunkedTruncated(body) => {
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n\r\n"
                );
                let _ = write!(stream, "{:x}\r\n", body.len());
                let _ = stream.write_all(&body);
                let _ = stream.write_all(b"\r\n");
                // No terminating zero-length chunk — the stream just stops.
            }
            Reply::ServerError => {
                let _ = write!(
                    stream,
                    "HTTP/1.1 500 Internal Server Error\r\nConnection: close\r\n\r\n"
                );
            }
            Reply::Unavailable(retry_after) => {
                let header = retry_after
                    .map(|s| format!("Retry-After: {s}\r\n"))
                    .unwrap_or_default();
                let _ = write!(
                    stream,
                    "HTTP/1.1 503 Service Unavailable\r\nConnection: close\r\n{header}Content-Length: 0\r\n\r\n"
                );
            }
        }
        // Every reply says `Connection: close` because of this. Left to assume
        // keep-alive, the client can send its next request down this socket
        // before the close reaches it, and that fails as a broken connection.
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
            download_with_retries(&dest, 1, None, || Ok(client.get(server.url())), |_, _| {})
                .unwrap();

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

        let err = download_with_retries(&dest, 1, None, || Ok(client.get(server.url())), |_, _| {})
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

        let err = download_with_retries(&dest, 1, None, || Ok(client.get(server.url())), |_, _| {})
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
            download_with_retries(&dest, 3, None, || Ok(client.get(server.url())), |_, _| {})
                .unwrap();

        assert_eq!(written, body.len() as u64);
        assert_eq!(server.hits(), 3, "should have used all three attempts");
        assert_eq!(std::fs::read(&dest).unwrap(), body);
    }

    /// A stream reading the `.part` holds its inode, so a retry has to write
    /// into that same file rather than a new one beside it.
    #[cfg(unix)]
    #[test]
    fn retry_rewrites_the_same_part_file() {
        use std::os::unix::fs::MetadataExt;

        let server = StubServer::start(vec![
            Reply::Truncated {
                claimed: 50_000,
                body: vec![3u8; 10],
            },
            Reply::Complete(vec![3u8; 50_000]),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let dest = tmp_dest(&dir);
        let client = download_client().unwrap();

        let inodes = std::sync::Mutex::new(std::collections::HashSet::new());
        download_with_retries(
            &dest,
            2,
            None,
            || Ok(client.get(server.url())),
            |_, _| {
                if let Ok(meta) = std::fs::metadata(part_path(&dest)) {
                    inodes.lock().unwrap().insert(meta.ino());
                }
            },
        )
        .unwrap();

        assert_eq!(inodes.lock().unwrap().len(), 1);
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
            None,
            || Ok(client.get(server.url())),
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

    fn status(code: u16) -> DownloadError {
        DownloadError::Status {
            status: reqwest::StatusCode::from_u16(code).unwrap(),
            retry_after: None,
        }
    }

    #[test]
    fn an_unanswering_server_is_told_apart_from_a_bad_track() {
        for code in [503, 429, 502, 504] {
            assert!(status(code).is_unavailable(), "{code} is the server");
        }
        for code in [404, 500, 403] {
            assert!(!status(code).is_unavailable(), "{code} is the track");
        }
        assert!(
            !DownloadError::Incomplete {
                got: 1,
                expected: 2
            }
            .is_unavailable()
        );
        assert!(!DownloadError::Request("error document".into()).is_unavailable());
    }

    #[test]
    fn a_refused_connection_is_the_server_being_unavailable() {
        // A port nothing listens on.
        let addr = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let err = download_client()
            .unwrap()
            .get(format!("http://{addr}/file"))
            .send()
            .map(|_| ())
            .map_err(DownloadError::from)
            .expect_err("nothing is listening");
        assert!(err.is_unavailable(), "unexpected: {err}");
    }

    #[test]
    fn outage_backoff_steps_then_holds_and_defers_to_retry_after() {
        let s = Duration::from_secs;
        let schedule: Vec<_> = (1..=5).map(|n| outage_backoff(n, None)).collect();
        assert_eq!(schedule, [s(5), s(15), s(60), s(60), s(60)]);
        assert_eq!(outage_backoff(1, Some(s(2))), s(2));
        assert_eq!(outage_backoff(4, Some(s(0))), s(0));
        assert_eq!(outage_backoff(1, Some(s(86_400))), RETRY_AFTER_CAP);
    }

    fn patient<'a>(outage: &'a Outage, cancelled: &'a dyn Fn() -> bool) -> Option<Patience<'a>> {
        Some(Patience { outage, cancelled })
    }

    #[test]
    fn retry_after_is_read_and_honoured() {
        let server = StubServer::start(vec![
            Reply::Unavailable(Some(1)),
            Reply::Complete(vec![1u8; 1_000]),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let dest = tmp_dest(&dir);
        let client = download_client().unwrap();
        let outage = Outage::default();

        let started = Instant::now();
        download_with_retries(
            &dest,
            1,
            patient(&outage, &|| false),
            || Ok(client.get(server.url())),
            |_, _| {},
        )
        .unwrap();

        assert!(
            started.elapsed() >= Duration::from_secs(1),
            "waited as asked"
        );
        assert!(
            started.elapsed() < OUTAGE_BACKOFF[0],
            "not the default wait"
        );
        assert!(!outage.is_down(), "a success says the server is back");
    }

    #[test]
    fn an_outage_does_not_spend_the_tracks_attempts() {
        // Nine unanswered tries against one attempt: none of them is the
        // track's fault, so none of them counts.
        let mut replies = vec![Reply::Unavailable(Some(0)); 9];
        replies.push(Reply::Complete(vec![2u8; 1_000]));
        let server = StubServer::start(replies);
        let dir = tempfile::tempdir().unwrap();
        let dest = tmp_dest(&dir);
        let client = download_client().unwrap();

        download_with_retries(
            &dest,
            1,
            patient(&Outage::default(), &|| false),
            || Ok(client.get(server.url())),
            |_, _| {},
        )
        .unwrap();
        assert_eq!(server.hits(), 10);
    }

    #[test]
    fn without_patience_an_outage_fails_as_before() {
        let server = StubServer::start(vec![Reply::Unavailable(Some(0))]);
        let dir = tempfile::tempdir().unwrap();
        let dest = tmp_dest(&dir);
        let client = download_client().unwrap();

        let err = download_with_retries(&dest, 2, None, || Ok(client.get(server.url())), |_, _| {})
            .expect_err("bounded attempts");
        assert!(err.is_unavailable());
        assert_eq!(server.hits(), 2);
    }

    #[test]
    fn a_download_waiting_out_an_outage_stops_when_no_longer_wanted() {
        let server = StubServer::start(vec![Reply::Unavailable(None)]);
        let dir = tempfile::tempdir().unwrap();
        let dest = tmp_dest(&dir);
        let client = download_client().unwrap();
        let outage = Outage::default();
        let wanted = std::sync::atomic::AtomicBool::new(true);

        let started = Instant::now();
        let err = std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(200));
                wanted.store(false, Ordering::Relaxed);
            });
            download_with_retries(
                &dest,
                3,
                patient(&outage, &|| !wanted.load(Ordering::Relaxed)),
                || Ok(client.get(server.url())),
                |_, _| {},
            )
            .expect_err("cancelled")
        });

        assert!(matches!(err, DownloadError::Cancelled), "unexpected: {err}");
        assert!(
            started.elapsed() < OUTAGE_BACKOFF[0],
            "stopped within a poll, not at the next try"
        );
        assert_eq!(server.hits(), 1);
        assert!(!part_path(&dest).exists());
    }

    #[test]
    fn hold_blocks_while_down_and_lets_go_when_the_server_answers() {
        let outage = Outage::default();
        outage.down(Some(Duration::from_secs(60)));
        assert!(outage.is_down());

        let started = Instant::now();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(100));
                outage.up();
            });
            outage.hold();
        });
        let waited = started.elapsed();
        assert!(waited >= Duration::from_millis(100), "held while down");
        assert!(waited < Duration::from_secs(5), "released by the answer");
    }

    /// The queue's discipline — each worker holds while the server is down,
    /// then downloads — against a server that answers nothing for a while.
    #[test]
    fn a_queue_against_an_unavailable_server_fails_nothing_and_resumes() {
        const TRACKS: usize = 6;
        const UNANSWERED: usize = 12;
        let mut replies = vec![Reply::Unavailable(Some(0)); UNANSWERED];
        replies.push(Reply::Complete(vec![5u8; 10_000]));
        let server = StubServer::start(replies);
        let dir = tempfile::tempdir().unwrap();
        let client = download_client().unwrap();
        let outage = Outage::default();

        let results: Vec<_> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..TRACKS)
                .map(|i| {
                    let (client, outage, server, dir) = (&client, &outage, &server, &dir);
                    scope.spawn(move || {
                        outage.hold();
                        let dest = dir.path().join(format!("{i}.flac"));
                        download_with_retries(
                            &dest,
                            DEFAULT_ATTEMPTS,
                            patient(outage, &|| false),
                            || Ok(client.get(server.url())),
                            |_, _| {},
                        )
                        .map(|_| dest)
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        for result in results {
            let dest = result.expect("no track fails for the server being down");
            assert_eq!(std::fs::read(dest).unwrap().len(), 10_000);
        }
        assert_eq!(server.hits(), UNANSWERED + TRACKS);
        assert!(!outage.is_down());
    }
}
