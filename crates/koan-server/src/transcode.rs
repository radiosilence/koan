//! Lossy transcoding for Subsonic `stream`, through an `ffmpeg` subprocess.
//!
//! A client asks for less than the original with `maxBitRate` (kbps) or
//! `format`. Without either, or with `format=raw`, the original is served, as
//! it always is from `download`. Where `ffmpeg` cannot be found the server
//! serves originals and says so once at startup.
//!
//! A transcode has no length until it ends, so its response carries none and
//! honours no Range. Clients seek with `timeOffset` instead (OpenSubsonic's
//! `transcodeOffset`).

use std::path::{Path, PathBuf};
use std::process::Stdio;

use axum::http::{StatusCode, header};
use axum::response::Response;

/// A lossy format the server encodes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    Opus,
    Mp3,
}

impl Codec {
    fn from_format(format: &str) -> Option<Self> {
        match format.to_ascii_lowercase().as_str() {
            "opus" => Some(Self::Opus),
            "mp3" => Some(Self::Mp3),
            _ => None,
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            Self::Opus => "audio/ogg",
            Self::Mp3 => "audio/mpeg",
        }
    }

    /// The bitrate used when the client names a format and no limit.
    fn default_kbps(self) -> u32 {
        match self {
            Self::Opus => 128,
            Self::Mp3 => 192,
        }
    }

    /// The range each encoder accepts, in kbps.
    fn kbps_range(self) -> (u32, u32) {
        match self {
            Self::Opus => (16, 256),
            Self::Mp3 => (32, 320),
        }
    }

    /// The source's codec, as the indexer records it, is this one.
    fn matches(self, source_codec: &str) -> bool {
        match self {
            Self::Opus => source_codec.eq_ignore_ascii_case("opus"),
            Self::Mp3 => source_codec.eq_ignore_ascii_case("mp3"),
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

/// The `ffmpeg` the server transcodes with, found at startup.
#[derive(Debug, Clone)]
pub struct Transcoder {
    ffmpeg: PathBuf,
}

impl Transcoder {
    /// The transcoder for `ffmpeg`, if it runs. Blocks for as long as
    /// `ffmpeg -version` takes, so it is called once, at startup.
    pub fn find(ffmpeg: &str) -> Option<Self> {
        let runs = std::process::Command::new(ffmpeg)
            .arg("-version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        runs.then(|| Self {
            ffmpeg: PathBuf::from(ffmpeg),
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

    /// The transcode of `path` as a response body, read from `ffmpeg`'s
    /// stdout as it encodes. The process is killed when the response is
    /// dropped, which is what a client hanging up does.
    pub fn stream(&self, path: &Path, plan: &Plan) -> std::io::Result<Response> {
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
        let stderr = child.stderr.take();
        let shown = path.display().to_string();
        tokio::spawn(async move {
            let mut errors = String::new();
            if let Some(mut stderr) = stderr {
                let _ = tokio::io::AsyncReadExt::read_to_string(&mut stderr, &mut errors).await;
            }
            // Killed for a client that left is not worth a warning.
            if let Ok(status) = child.wait().await
                && !status.success()
                && !errors.trim().is_empty()
            {
                log::warn!("transcode of {shown} failed: {}", errors.trim());
            }
        });
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, plan.codec.content_type())
            .header(header::ACCEPT_RANGES, "none")
            .body(axum::body::Body::from_stream(
                tokio_util::io::ReaderStream::with_capacity(stdout, 64 * 1024),
            ))
            .map_err(std::io::Error::other)
    }
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
        assert_eq!(plan(&req(None, Some("aac")), Some("FLAC"), Some(900)), None);
        let p = plan(&req(Some("128"), Some("aac")), Some("FLAC"), Some(900)).unwrap();
        assert_eq!(p.codec, Codec::Opus);
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
}
