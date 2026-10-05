//! Lossy transcoding for Subsonic `stream`, through an `ffmpeg` subprocess.
//!
//! A client asks for less than the original with `maxBitRate` (kbps) or
//! `format`. Without either, or with `format=raw`, the original is served, as
//! it always is from `download`. Where `ffmpeg` cannot be found the server
//! serves originals and says so once at startup.
//!
//! A transcode has no length until it ends, so its response carries none
//! unless the client asks for an estimate, and honours no Range. Clients seek
//! with `timeOffset` instead (OpenSubsonic's `transcodeOffset`). When the
//! server or the account is at its limit of running transcodes, the original
//! is served.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_stream::StreamExt;

use axum::http::{StatusCode, header};
use axum::response::Response;

/// A lossy format the server encodes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    Opus,
    Mp3,
    /// AAC-LC in ADTS, from ffmpeg's own encoder: libfdk_aac is non-free and
    /// absent from most builds.
    Aac,
}

impl Codec {
    fn from_format(format: &str) -> Option<Self> {
        match format.to_ascii_lowercase().as_str() {
            "opus" => Some(Self::Opus),
            "mp3" => Some(Self::Mp3),
            "aac" | "m4a" => Some(Self::Aac),
            _ => None,
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            Self::Opus => "audio/ogg",
            Self::Mp3 => "audio/mpeg",
            Self::Aac => "audio/aac",
        }
    }

    /// The bitrate used when the client names a format and no limit.
    fn default_kbps(self) -> u32 {
        match self {
            Self::Opus => 128,
            Self::Mp3 => 192,
            Self::Aac => 192,
        }
    }

    /// The range each encoder accepts, in kbps.
    fn kbps_range(self) -> (u32, u32) {
        match self {
            Self::Opus => (16, 256),
            Self::Mp3 => (32, 320),
            Self::Aac => (64, 320),
        }
    }

    /// The source's codec, as the indexer records it, is this one.
    fn matches(self, source_codec: &str) -> bool {
        match self {
            Self::Opus => source_codec.eq_ignore_ascii_case("opus"),
            Self::Mp3 => source_codec.eq_ignore_ascii_case("mp3"),
            Self::Aac => source_codec.eq_ignore_ascii_case("aac"),
        }
    }
}

/// What `stream` was asked for, as it arrived.
#[derive(Debug, Default, Clone)]
pub struct Request<'a> {
    pub max_bit_rate: Option<&'a str>,
    pub format: Option<&'a str>,
    pub time_offset: Option<&'a str>,
}

/// One transcode: what to encode to, at what bitrate, from where.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plan {
    pub codec: Codec,
    pub kbps: u32,
    pub offset_secs: f64,
}

/// Whether, and how, to transcode a track for a request. `None` serves the
/// original.
///
/// `source_codec` and `source_kbps` describe the file as indexed. A limit at
/// or above the source's bitrate keeps the original unless a different
/// format was asked for; a source whose bitrate is unknown counts as over any
/// limit. A format the server cannot encode to is ignored, which leaves the
/// limit, if there is one, to decide.
pub fn plan(
    request: &Request,
    source_codec: Option<&str>,
    source_kbps: Option<u32>,
) -> Option<Plan> {
    let format = request.format.map(str::trim).filter(|f| !f.is_empty());
    if format.is_some_and(|f| f.eq_ignore_ascii_case("raw")) {
        return None;
    }
    // Subsonic's `0` means no limit.
    let limit = request
        .max_bit_rate
        .and_then(|m| m.trim().parse::<u32>().ok())
        .filter(|&m| m > 0);
    let asked = format.and_then(Codec::from_format);
    let over_limit = limit.is_some_and(|l| source_kbps.is_none_or(|s| s > l));
    let already = asked.is_some_and(|c| source_codec.is_some_and(|s| c.matches(s)));

    let codec = match asked {
        Some(c) if !already => c,
        Some(c) if over_limit => c,
        Some(_) => return None,
        None if over_limit => Codec::Opus,
        None => return None,
    };
    let (lo, hi) = codec.kbps_range();
    let kbps = limit.unwrap_or_else(|| codec.default_kbps()).clamp(lo, hi);
    let offset_secs = request
        .time_offset
        .and_then(|t| t.trim().parse::<f64>().ok())
        .filter(|t| t.is_finite() && *t > 0.0)
        .unwrap_or(0.0);
    Some(Plan {
        codec,
        kbps,
        offset_secs,
    })
}

/// The length a transcode is said to have when a client asks for one
/// (`estimateContentLength`): its bitrate over what is left of the track.
pub fn estimated_length(plan: &Plan, duration_ms: Option<i64>) -> Option<u64> {
    let secs = (duration_ms? as f64 / 1000.0 - plan.offset_secs).max(0.0);
    Some((f64::from(plan.kbps) * 125.0 * secs).round() as u64)
}

/// Transcodes running at once, across all accounts. Each is an encoder
/// pinning a core.
fn global_limit() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get().max(2))
}

/// Transcodes running at once for one account: a track and the next one a
/// client fetches ahead, from a device or two.
const PER_ACCOUNT: usize = 3;

/// How much of ffmpeg's error output is kept to log a failure with.
const STDERR_TAIL: usize = 4096;

/// What one running transcode holds of the limits; released with the body.
pub struct Permits {
    _global: OwnedSemaphorePermit,
    _account: OwnedSemaphorePermit,
}

/// The `ffmpeg` the server transcodes with, found at startup, and the limits
/// on how many it runs.
pub struct Transcoder {
    ffmpeg: PathBuf,
    opus: bool,
    mp3: bool,
    aac: bool,
    global: Arc<Semaphore>,
    accounts: Mutex<HashMap<String, Arc<Semaphore>>>,
}

impl Transcoder {
    /// The transcoder for `ffmpeg`, if it runs and has an encoder for at
    /// least one of the codecs. Blocks for as long as `ffmpeg -encoders`
    /// takes, so it is called once, at startup.
    pub fn find(ffmpeg: &str) -> Option<Self> {
        let out = std::process::Command::new(ffmpeg)
            .args(["-hide_banner", "-encoders"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())?;
        let listed = String::from_utf8_lossy(&out.stdout);
        let has = |name: &str| listed.split_whitespace().any(|w| w == name);
        let (opus, mp3, aac) = (has("libopus"), has("libmp3lame"), has("aac"));
        if !opus && !mp3 && !aac {
            log::info!("Subsonic: {ffmpeg} has none of libopus, libmp3lame and aac.");
            return None;
        }
        Some(Self {
            ffmpeg: PathBuf::from(ffmpeg),
            opus,
            mp3,
            aac,
            global: Arc::new(Semaphore::new(global_limit())),
            accounts: Mutex::new(HashMap::new()),
        })
    }

    /// Whether this ffmpeg can encode to `codec`.
    pub fn encodes(&self, codec: Codec) -> bool {
        match codec {
            Codec::Opus => self.opus,
            Codec::Mp3 => self.mp3,
            Codec::Aac => self.aac,
        }
    }

    /// Room for one more transcode for `account`, or `None` when the server
    /// or the account is at its limit.
    pub fn permits(&self, account: &str) -> Option<Permits> {
        let account = self
            .accounts
            .lock()
            .entry(account.to_owned())
            .or_insert_with(|| Arc::new(Semaphore::new(PER_ACCOUNT)))
            .clone()
            .try_acquire_owned()
            .ok()?;
        let global = self.global.clone().try_acquire_owned().ok()?;
        Some(Permits {
            _global: global,
            _account: account,
        })
    }

    fn args(path: &Path, plan: &Plan) -> Vec<std::ffi::OsString> {
        let mut args: Vec<std::ffi::OsString> =
            vec!["-nostdin".into(), "-loglevel".into(), "error".into()];
        if plan.offset_secs > 0.0 {
            args.extend(["-ss".into(), format!("{:.3}", plan.offset_secs).into()]);
        }
        args.extend(["-i".into(), path.as_os_str().to_owned()]);
        // The first audio stream only: embedded cover art is a video stream.
        args.extend([
            "-map".into(),
            "0:a:0".into(),
            "-map_metadata".into(),
            "-1".into(),
        ]);
        let (encoder, muxer) = match plan.codec {
            Codec::Opus => ("libopus", "ogg"),
            Codec::Mp3 => ("libmp3lame", "mp3"),
            Codec::Aac => ("aac", "adts"),
        };
        args.extend([
            "-c:a".into(),
            encoder.into(),
            "-b:a".into(),
            format!("{}k", plan.kbps).into(),
            "-f".into(),
            muxer.into(),
            "-".into(),
        ]);
        args
    }

    /// The headers a transcode would be sent with, for a HEAD request, which
    /// runs no encoder. The body is a stream of nothing rather than an empty
    /// body: an empty body has a known size, and would be announced as a
    /// `Content-Length` of zero.
    pub fn head(plan: &Plan, length: Option<u64>) -> std::io::Result<Response> {
        let nothing = tokio_stream::empty::<std::io::Result<axum::body::Bytes>>();
        Self::response(plan, length, axum::body::Body::from_stream(nothing))
    }

    fn response(
        plan: &Plan,
        length: Option<u64>,
        body: axum::body::Body,
    ) -> std::io::Result<Response> {
        let mut builder = Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, plan.codec.content_type())
            .header(header::ACCEPT_RANGES, "none");
        if let Some(length) = length {
            builder = builder.header(header::CONTENT_LENGTH, length);
        }
        builder.body(body).map_err(std::io::Error::other)
    }

    /// The transcode of `path` as a response body, read from ffmpeg's stdout
    /// as it encodes, or `None` when ffmpeg ends before encoding anything, for
    /// the caller to serve the original instead.
    ///
    /// The body owns the process and the permits: a client hanging up drops
    /// it, which kills ffmpeg and frees the slot. With `length`, the body is
    /// cut or padded with zeros to exactly that many bytes, since a client
    /// that asked for an estimate was promised it.
    pub async fn stream(
        &self,
        path: &Path,
        plan: &Plan,
        length: Option<u64>,
        permits: Permits,
    ) -> std::io::Result<Option<Response>> {
        let mut child = tokio::process::Command::new(&self.ffmpeg)
            .args(Self::args(path, plan))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("ffmpeg has no stdout"))?;
        let stderr = child.stderr.take().map(|s| tokio::spawn(stderr_tail(s)));
        let shown = path.display().to_string();
        let mut chunks = tokio_util::io::ReaderStream::with_capacity(stdout, 64 * 1024);

        let first = match chunks.next().await {
            Some(Ok(chunk)) if !chunk.is_empty() => chunk,
            _ => {
                let status = child.wait().await.ok();
                let errors = match stderr {
                    Some(task) => task.await.unwrap_or_default(),
                    None => String::new(),
                };
                log::warn!(
                    "transcode of {shown} produced nothing ({}), serving the original: {}",
                    status.map_or("not waited".into(), |s| s.to_string()),
                    errors.trim()
                );
                return Ok(None);
            }
        };

        let body = async_stream::stream! {
            let _permits = permits;
            let mut child = child;
            let mut sent: u64 = 0;
            let mut pending = Some(first);
            loop {
                let chunk = match pending.take() {
                    Some(chunk) => chunk,
                    None => match chunks.next().await {
                        Some(Ok(chunk)) => chunk,
                        Some(Err(e)) => {
                            yield Err(e);
                            return;
                        }
                        None => break,
                    },
                };
                let chunk = match length {
                    Some(n) if sent + chunk.len() as u64 >= n => {
                        let cut = chunk.slice(..(n - sent) as usize);
                        if !cut.is_empty() {
                            yield Ok(cut);
                        }
                        // Promised length reached: the rest is not wanted.
                        return;
                    }
                    _ => chunk,
                };
                sent += chunk.len() as u64;
                yield Ok::<_, std::io::Error>(chunk);
            }
            // ffmpeg closed its output by itself; a hang-up never gets here.
            if let Ok(status) = child.wait().await
                && !status.success()
            {
                let errors = match stderr {
                    Some(task) => task.await.unwrap_or_default(),
                    None => String::new(),
                };
                log::warn!("transcode of {shown} failed ({status}): {}", errors.trim());
            }
            if let Some(n) = length {
                while sent < n {
                    let pad = (n - sent).min(64 * 1024) as usize;
                    sent += pad as u64;
                    yield Ok(axum::body::Bytes::from(vec![0u8; pad]));
                }
            }
        };
        Self::response(plan, length, axum::body::Body::from_stream(body)).map(Some)
    }
}

/// The last `STDERR_TAIL` bytes ffmpeg writes to stderr.
async fn stderr_tail(mut stderr: tokio::process::ChildStderr) -> String {
    use tokio::io::AsyncReadExt;
    let mut tail = Vec::new();
    let mut buf = [0u8; 1024];
    while let Ok(n) = stderr.read(&mut buf).await {
        if n == 0 {
            break;
        }
        tail.extend_from_slice(&buf[..n]);
        if tail.len() > STDERR_TAIL {
            tail.drain(..tail.len() - STDERR_TAIL);
        }
    }
    String::from_utf8_lossy(&tail).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req<'a>(max: Option<&'a str>, format: Option<&'a str>) -> Request<'a> {
        Request {
            max_bit_rate: max,
            format,
            time_offset: None,
        }
    }

    #[test]
    fn nothing_asked_serves_the_original() {
        assert_eq!(plan(&req(None, None), Some("FLAC"), Some(900)), None);
        assert_eq!(plan(&req(Some("0"), None), Some("FLAC"), Some(900)), None);
    }

    #[test]
    fn raw_serves_the_original_whatever_the_limit() {
        assert_eq!(
            plan(&req(Some("128"), Some("raw")), Some("FLAC"), Some(900)),
            None
        );
    }

    #[test]
    fn a_limit_under_the_source_transcodes_to_opus_at_the_limit() {
        let p = plan(&req(Some("96"), None), Some("FLAC"), Some(900)).unwrap();
        assert_eq!((p.codec, p.kbps), (Codec::Opus, 96));
    }

    #[test]
    fn a_limit_over_the_source_serves_the_original() {
        assert_eq!(plan(&req(Some("320"), None), Some("MP3"), Some(256)), None);
    }

    #[test]
    fn an_unknown_source_bitrate_counts_as_over_the_limit() {
        assert!(plan(&req(Some("320"), None), Some("FLAC"), None).is_some());
    }

    #[test]
    fn a_named_format_transcodes_at_its_default() {
        let p = plan(&req(None, Some("mp3")), Some("FLAC"), Some(900)).unwrap();
        assert_eq!((p.codec, p.kbps), (Codec::Mp3, 192));
        let p = plan(&req(None, Some("opus")), Some("FLAC"), Some(900)).unwrap();
        assert_eq!((p.codec, p.kbps), (Codec::Opus, 128));
    }

    #[test]
    fn the_source_format_under_the_limit_serves_the_original() {
        assert_eq!(plan(&req(None, Some("mp3")), Some("MP3"), Some(320)), None);
        assert_eq!(
            plan(&req(Some("320"), Some("mp3")), Some("MP3"), Some(256)),
            None
        );
        let p = plan(&req(Some("128"), Some("mp3")), Some("MP3"), Some(320)).unwrap();
        assert_eq!((p.codec, p.kbps), (Codec::Mp3, 128));
    }

    #[test]
    fn an_unsupported_format_leaves_the_limit_to_decide() {
        assert_eq!(plan(&req(None, Some("wma")), Some("FLAC"), Some(900)), None);
        let p = plan(&req(Some("128"), Some("wma")), Some("FLAC"), Some(900)).unwrap();
        assert_eq!(p.codec, Codec::Opus);
    }

    #[test]
    fn aac_is_asked_for_as_aac_or_m4a() {
        for format in ["aac", "m4a", "AAC"] {
            let p = plan(&req(None, Some(format)), Some("FLAC"), Some(900)).unwrap();
            assert_eq!((p.codec, p.kbps), (Codec::Aac, 192));
        }
        let p = plan(&req(Some("32"), Some("aac")), Some("FLAC"), Some(900)).unwrap();
        assert_eq!(p.kbps, 64);
        let p = plan(&req(Some("128"), Some("aac")), Some("FLAC"), Some(900)).unwrap();
        assert_eq!((p.codec, p.kbps), (Codec::Aac, 128));
    }

    #[test]
    fn aac_within_the_limit_passes_through() {
        assert_eq!(plan(&req(None, Some("aac")), Some("AAC"), Some(256)), None);
        assert_eq!(
            plan(&req(Some("320"), Some("m4a")), Some("AAC"), Some(256)),
            None
        );
        let p = plan(&req(Some("128"), Some("aac")), Some("AAC"), Some(256)).unwrap();
        assert_eq!((p.codec, p.kbps), (Codec::Aac, 128));
        // ALAC sits in the same container and is not AAC.
        assert!(plan(&req(None, Some("m4a")), Some("ALAC"), Some(900)).is_some());
    }

    #[test]
    fn aac_is_muxed_as_adts_by_the_native_encoder() {
        let plan = Plan {
            codec: Codec::Aac,
            kbps: 128,
            offset_secs: 0.0,
        };
        let args: Vec<String> = Transcoder::args(Path::new("/m/a.flac"), &plan)
            .into_iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(args.windows(2).any(|w| w == ["-c:a", "aac"]));
        assert!(args.windows(2).any(|w| w == ["-f", "adts"]));
    }

    #[test]
    fn bitrates_are_clamped_to_the_encoder() {
        let p = plan(&req(Some("8"), None), Some("FLAC"), Some(900)).unwrap();
        assert_eq!(p.kbps, 16);
        let p = plan(&req(Some("500"), Some("mp3")), Some("FLAC"), Some(900)).unwrap();
        assert_eq!(p.kbps, 320);
    }

    #[test]
    fn time_offset_is_read_and_junk_ignored() {
        let mut r = req(Some("128"), None);
        r.time_offset = Some("42.5");
        assert_eq!(plan(&r, Some("FLAC"), Some(900)).unwrap().offset_secs, 42.5);
        r.time_offset = Some("soon");
        assert_eq!(plan(&r, Some("FLAC"), Some(900)).unwrap().offset_secs, 0.0);
        r.time_offset = Some("-3");
        assert_eq!(plan(&r, Some("FLAC"), Some(900)).unwrap().offset_secs, 0.0);
    }

    #[test]
    fn ffmpeg_is_told_to_seek_before_its_input() {
        let plan = Plan {
            codec: Codec::Mp3,
            kbps: 128,
            offset_secs: 30.0,
        };
        let args: Vec<String> = Transcoder::args(Path::new("/m/a.flac"), &plan)
            .into_iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let ss = args.iter().position(|a| a == "-ss").unwrap();
        let input = args.iter().position(|a| a == "-i").unwrap();
        assert!(ss < input);
        assert_eq!(args[ss + 1], "30.000");
        assert!(args.windows(2).any(|w| w == ["-c:a", "libmp3lame"]));
        assert!(args.windows(2).any(|w| w == ["-b:a", "128k"]));
        assert_eq!(args.last().unwrap(), "-");
    }

    #[test]
    fn the_estimate_is_the_bitrate_over_what_is_left() {
        let plan = Plan {
            codec: Codec::Opus,
            kbps: 128,
            offset_secs: 10.0,
        };
        assert_eq!(estimated_length(&plan, Some(70_000)), Some(128 * 125 * 60));
        assert_eq!(estimated_length(&plan, Some(5_000)), Some(0));
        assert_eq!(estimated_length(&plan, None), None);
    }

    fn transcoder(global: usize) -> Transcoder {
        Transcoder {
            ffmpeg: PathBuf::from("ffmpeg"),
            opus: true,
            mp3: true,
            aac: true,
            global: Arc::new(Semaphore::new(global)),
            accounts: Mutex::new(HashMap::new()),
        }
    }

    #[test]
    fn an_account_at_its_limit_gets_no_permit_until_one_is_released() {
        let t = transcoder(16);
        let held: Vec<_> = (0..PER_ACCOUNT).map(|_| t.permits("a").unwrap()).collect();
        assert!(t.permits("a").is_none());
        assert!(t.permits("b").is_some());
        drop(held);
        assert!(t.permits("a").is_some());
    }

    #[test]
    fn the_server_limit_holds_across_accounts() {
        let t = transcoder(2);
        let _a = t.permits("a").unwrap();
        let _b = t.permits("b").unwrap();
        assert!(t.permits("c").is_none());
        // A refused global permit gives the account's back.
        drop(_a);
        assert!(t.permits("c").is_some());
    }

    /// A real ffmpeg, where one is installed with the encoders.
    fn real() -> Option<Transcoder> {
        Transcoder::find("ffmpeg")
    }

    fn sine(dir: &Path) -> Option<PathBuf> {
        let path = dir.join("sine.flac");
        std::process::Command::new("ffmpeg")
            .args(["-nostdin", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=2")
            .arg(&path)
            .status()
            .ok()?
            .success()
            .then_some(path)
    }

    async fn body_len(resp: Response) -> usize {
        axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .len()
    }

    #[tokio::test]
    async fn a_promised_length_is_met_exactly() {
        let Some(t) = real() else { return };
        let dir = tempfile::tempdir().unwrap();
        let Some(path) = sine(dir.path()) else { return };
        let plan = Plan {
            codec: if t.opus { Codec::Opus } else { Codec::Mp3 },
            kbps: 64,
            offset_secs: 0.0,
        };
        for promised in [1_000u64, 1_000_000] {
            let permits = t.permits("a").unwrap();
            let resp = t
                .stream(&path, &plan, Some(promised), permits)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                resp.headers()[header::CONTENT_LENGTH],
                promised.to_string().as_str()
            );
            assert_eq!(body_len(resp).await as u64, promised);
        }
    }

    #[tokio::test]
    async fn a_file_ffmpeg_cannot_read_falls_back_to_the_original() {
        let Some(t) = real() else { return };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("junk.flac");
        std::fs::write(&path, b"not audio").unwrap();
        let plan = Plan {
            codec: if t.opus { Codec::Opus } else { Codec::Mp3 },
            kbps: 64,
            offset_secs: 0.0,
        };
        let permits = t.permits("a").unwrap();
        assert!(
            t.stream(&path, &plan, None, permits)
                .await
                .unwrap()
                .is_none()
        );
        // The permit went with the failed attempt.
        let held: Vec<_> = (0..PER_ACCOUNT).map(|_| t.permits("a")).collect();
        assert!(held.iter().all(Option::is_some));
    }

    #[test]
    fn a_head_names_no_length_unless_one_was_estimated() {
        use axum::body::HttpBody;
        let plan = Plan {
            codec: Codec::Mp3,
            kbps: 128,
            offset_secs: 0.0,
        };
        let head = Transcoder::head(&plan, None).unwrap();
        assert!(head.headers().get(header::CONTENT_LENGTH).is_none());
        assert_eq!(head.body().size_hint().exact(), None);
        let head = Transcoder::head(&plan, Some(4000)).unwrap();
        assert_eq!(head.headers()[header::CONTENT_LENGTH], "4000");
    }
}
