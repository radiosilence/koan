//! Streaming and downloads: local files with ranges, transcodes, and
//! tracks proxied from the upstream server.

use super::*;

#[derive(Debug, Deserialize)]
pub(super) struct StreamParams {
    #[serde(flatten)]
    pub(super) auth: SubsonicParams,
    /// A `String`, not an `i64`: axum's `Query` rejects a value it cannot
    /// deserialise with a plain-text HTTP 400 *before* the handler runs, which
    /// is neither a Subsonic envelope nor something a client can report.
    pub(super) id: Option<String>,
    /// Strings for the same reason as `id`. Read only by `stream`; `download`
    /// is the original file by definition.
    #[serde(rename = "maxBitRate")]
    pub(super) max_bit_rate: Option<String>,
    pub(super) format: Option<String>,
    #[serde(rename = "timeOffset")]
    pub(super) time_offset: Option<String>,
    #[serde(rename = "estimateContentLength")]
    pub(super) estimate_content_length: Option<String>,
}

/// What `stream_inner` may send in place of the file itself.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Delivery {
    /// The original, always: `download`.
    Original,
    /// A transcode where the request asks for one.
    Transcode,
    /// The headers a transcode would have, without running one: `stream`'s HEAD.
    TranscodeHead,
}

pub(super) async fn stream(
    State(state): State<Arc<AppState>>,
    method: axum::http::Method,
    Query(params): Query<StreamParams>,
    headers: HeaderMap,
) -> Response {
    let json = params.auth.wants_json();
    let delivery = if method == axum::http::Method::HEAD {
        Delivery::TranscodeHead
    } else {
        Delivery::Transcode
    };
    match stream_inner(state, params, &headers, delivery).await {
        Ok(resp) => resp,
        Err(e) => SubsonicResponse::error(json, &e),
    }
}

pub(super) async fn download(
    State(state): State<Arc<AppState>>,
    Query(params): Query<StreamParams>,
    headers: HeaderMap,
) -> Response {
    let json = params.auth.wants_json();
    match stream_inner(state, params, &headers, Delivery::Original).await {
        Ok(resp) => resp,
        Err(e) => SubsonicResponse::error(json, &e),
    }
}

pub(super) async fn stream_inner(
    state: Arc<AppState>,
    params: StreamParams,
    headers: &HeaderMap,
    delivery: Delivery,
) -> Result<Response, SubsonicError> {
    let lookup = state.clone();
    let StreamParams {
        auth,
        id,
        max_bit_rate,
        format,
        time_offset,
        estimate_content_length,
    } = params;
    let (username, track) = offload(move || {
        let caller = validate_auth(&auth, &lookup)?;
        let db = lookup.open_db()?;
        let track_id = require_id(&db, id.as_deref(), EntityKind::Song)?;
        let track = queries::get_track_row(&db.conn, track_id)
            .map_err(|e| SubsonicError::internal(e.to_string()))?
            .ok_or_else(|| SubsonicError::not_found("Track"))?;
        Ok((caller.username, track))
    })
    .await?;

    // Try local/cached file first; fall back to proxying from upstream.
    let local_path = track_file_path(&track).map(PathBuf::from);
    let local_exists = if let Some(ref p) = local_path {
        tokio::fs::metadata(p).await.is_ok()
    } else {
        false
    };

    if !local_exists {
        // Proxy from upstream Navidrome/Subsonic server.
        if let Some(ref remote_id) = track.remote_id {
            return proxy_stream_from_upstream(&state, remote_id, &track, headers).await;
        }
        return Err(SubsonicError::not_found(
            "Track has no local file and no remote source",
        ));
    }

    let path = local_path.unwrap();
    if delivery != Delivery::Original
        && let Some(transcoder) = &state.transcoder
    {
        let request = crate::transcode::Request {
            max_bit_rate: max_bit_rate.as_deref(),
            format: format.as_deref(),
            time_offset: time_offset.as_deref(),
        };
        let source_kbps = track.bitrate.and_then(|b| u32::try_from(b).ok());
        let plan = crate::transcode::plan(&request, track.codec.as_deref(), source_kbps)
            .filter(|plan| transcoder.encodes(plan.codec));
        if let Some(plan) = plan {
            let length = (estimate_content_length.as_deref() == Some("true"))
                .then(|| crate::transcode::estimated_length(&plan, track.duration_ms))
                .flatten();
            if delivery == Delivery::TranscodeHead {
                return crate::transcode::Transcoder::head(&plan, length)
                    .map_err(|e| SubsonicError::internal(e.to_string()));
            }
            match transcoder.permits(&username) {
                None => log::info!("transcode: at the limit for {username}"),
                Some(permits) => match transcoder.stream(&path, &plan, length, permits).await {
                    Ok(Some(resp)) => return Ok(resp),
                    Ok(None) => {}
                    Err(e) => {
                        log::warn!("transcode: could not start ffmpeg: {e}")
                    }
                },
            }
            // The original is served in the transcode's place, but only from
            // its start: a client that asked for an offset would show one
            // position and play another.
            if plan.offset_secs > 0.0 {
                return Err(SubsonicError::generic(
                    "the stream could not be transcoded from an offset; try again",
                ));
            }
        }
    }
    serve_local_file(&path, headers).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            SubsonicError::not_found("File not found on disk")
        } else {
            SubsonicError::internal(e.to_string())
        }
    })
}

/// How much of a file each read takes. `tokio::fs` runs every read on the
/// blocking pool, and the 4 KiB default made a 500 MB album download some
/// 128,000 of them.
pub(super) const STREAM_CHUNK: usize = 256 * 1024;

/// A file from disk, honouring a Range header so players can seek. Shared by
/// Subsonic's `stream` and the public share pages.
pub(crate) async fn serve_local_file(
    path: &std::path::Path,
    headers: &HeaderMap,
) -> std::io::Result<Response> {
    let total_size = tokio::fs::metadata(path).await?.len();
    let content_type = path
        .extension()
        .and_then(|e| e.to_str())
        .map(extension_to_mime)
        .unwrap_or("application/octet-stream");
    let built = |r: Result<Response, axum::http::Error>| r.map_err(std::io::Error::other);

    if let Some(range_str) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) {
        match parse_range(range_str, total_size) {
            RangeRequest::Satisfiable { start, end } => {
                let length = end - start + 1;
                let mut file = tokio::fs::File::open(path).await?;
                tokio::io::AsyncSeekExt::seek(&mut file, std::io::SeekFrom::Start(start)).await?;
                let stream =
                    tokio_util::io::ReaderStream::with_capacity(file.take(length), STREAM_CHUNK);
                return built(
                    Response::builder()
                        .status(StatusCode::PARTIAL_CONTENT)
                        .header(header::CONTENT_TYPE, content_type)
                        .header(header::CONTENT_LENGTH, length)
                        .header(
                            header::CONTENT_RANGE,
                            format!("bytes {}-{}/{}", start, end, total_size),
                        )
                        .header(header::ACCEPT_RANGES, "bytes")
                        .body(axum::body::Body::from_stream(stream)),
                );
            }
            RangeRequest::Unsatisfiable => {
                return built(
                    Response::builder()
                        .status(StatusCode::RANGE_NOT_SATISFIABLE)
                        .header(header::CONTENT_RANGE, format!("bytes */{}", total_size))
                        .header(header::ACCEPT_RANGES, "bytes")
                        .body(axum::body::Body::empty()),
                );
            }
            // A header that does not parse is ignored and the whole body sent.
            RangeRequest::Malformed => {}
        }
    }

    let file = tokio::fs::File::open(path).await?;
    built(
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type)
            .header(header::CONTENT_LENGTH, total_size)
            .header(header::ACCEPT_RANGES, "bytes")
            .body(axum::body::Body::from_stream(
                tokio_util::io::ReaderStream::with_capacity(file, STREAM_CHUNK),
            )),
    )
}

/// Proxy a stream from the upstream Navidrome/Subsonic server.
/// Forwards the audio bytes through to the client, passing along Range headers.
pub(super) async fn proxy_stream_from_upstream(
    state: &AppState,
    remote_id: &str,
    track: &queries::TrackRow,
    client_headers: &HeaderMap,
) -> Result<Response, SubsonicError> {
    let upstream_url = state
        .upstream
        .as_ref()
        .ok_or_else(|| SubsonicError::not_found("Remote server not configured"))?
        .stream_url(remote_id)
        .map_err(|e| SubsonicError::internal(e.to_string()))?;

    let mut req = state.http.get(&upstream_url);

    if let Some(range) = client_headers.get(header::RANGE)
        && let Ok(range_str) = range.to_str()
    {
        req = req.header("Range", range_str);
    }

    // reqwest's error names the URL, and the URL carries the upstream
    // account's credentials: never let it reach the client or the log.
    let upstream_resp = req.send().await.map_err(|e| {
        log::warn!("stream proxy: upstream request failed: {}", e.without_url());
        SubsonicError::generic("Upstream server unavailable")
    })?;

    let status = upstream_resp.status();
    let content_type = track
        .codec
        .as_deref()
        .map(|c| codec_to_mime(c).1)
        .unwrap_or("application/octet-stream");

    let mut builder = Response::builder().status(status.as_u16());
    builder = builder.header(header::CONTENT_TYPE, content_type);

    // Forward content-length and range headers from upstream.
    if let Some(cl) = upstream_resp.headers().get(header::CONTENT_LENGTH) {
        builder = builder.header(header::CONTENT_LENGTH, cl);
    }
    if let Some(cr) = upstream_resp.headers().get(header::CONTENT_RANGE) {
        builder = builder.header(header::CONTENT_RANGE, cr);
    }
    builder = builder.header(header::ACCEPT_RANGES, "bytes");

    let body = axum::body::Body::from_stream(upstream_resp.bytes_stream());
    builder
        .body(body)
        .map_err(|e| SubsonicError::internal(e.to_string()))
}

/// What a `Range` header asks for.
///
/// RFC 9110 draws a line a bare `Option` could not: a header that does not
/// parse is ignored and the whole body sent, while a well-formed range outside
/// the file is a 416 carrying `Content-Range: bytes */<total>`.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum RangeRequest {
    Satisfiable { start: u64, end: u64 },
    Unsatisfiable,
    Malformed,
}

pub(super) fn parse_range(range: &str, total: u64) -> RangeRequest {
    let Some(spec) = range.strip_prefix("bytes=") else {
        return RangeRequest::Malformed;
    };
    let Some((start_str, end_str)) = spec.split_once('-') else {
        return RangeRequest::Malformed;
    };
    let (start_str, end_str) = (start_str.trim(), end_str.trim());

    if start_str.is_empty() {
        let Ok(suffix) = end_str.parse::<u64>() else {
            return RangeRequest::Malformed;
        };
        if total == 0 || suffix == 0 {
            return RangeRequest::Unsatisfiable;
        }
        return RangeRequest::Satisfiable {
            start: total.saturating_sub(suffix),
            end: total - 1,
        };
    }

    let Ok(start) = start_str.parse::<u64>() else {
        return RangeRequest::Malformed;
    };
    if total == 0 {
        return RangeRequest::Unsatisfiable;
    }
    let end = if end_str.is_empty() {
        total - 1
    } else {
        let Ok(end) = end_str.parse::<u64>() else {
            return RangeRequest::Malformed;
        };
        end.min(total - 1)
    };
    if start > end || start >= total {
        return RangeRequest::Unsatisfiable;
    }
    RangeRequest::Satisfiable { start, end }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subsonic::testing::*;
    use axum::http::Request;

    async fn get_with_range(
        app: axum::Router,
        uri: &str,
        range: &str,
    ) -> (StatusCode, HeaderMap, Vec<u8>) {
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header(header::RANGE, range)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, headers, body.to_vec())
    }

    #[test]
    fn test_parse_range_full() {
        assert_eq!(
            parse_range("bytes=0-999", 5000),
            RangeRequest::Satisfiable { start: 0, end: 999 }
        );
    }

    #[test]
    fn test_parse_range_open_end() {
        assert_eq!(
            parse_range("bytes=1000-", 5000),
            RangeRequest::Satisfiable {
                start: 1000,
                end: 4999
            }
        );
    }

    #[test]
    fn test_parse_range_suffix() {
        assert_eq!(
            parse_range("bytes=-500", 5000),
            RangeRequest::Satisfiable {
                start: 4500,
                end: 4999
            }
        );
    }

    #[test]
    fn test_parse_range_out_of_bounds() {
        assert_eq!(
            parse_range("bytes=5000-6000", 5000),
            RangeRequest::Unsatisfiable
        );
    }

    #[test]
    fn test_parse_range_clamps_end() {
        assert_eq!(
            parse_range("bytes=4000-9999", 5000),
            RangeRequest::Satisfiable {
                start: 4000,
                end: 4999
            }
        );
    }

    #[test]
    fn test_parse_range_on_empty_file() {
        assert_eq!(parse_range("bytes=0-", 0), RangeRequest::Unsatisfiable);
        assert_eq!(parse_range("bytes=-100", 0), RangeRequest::Unsatisfiable);
    }

    #[test]
    fn test_parse_range_malformed_is_ignored() {
        assert_eq!(parse_range("seconds=0-10", 5000), RangeRequest::Malformed);
        assert_eq!(parse_range("bytes=abc-def", 5000), RangeRequest::Malformed);
        // Multipart ranges are unsupported; serving the whole body is legal.
        assert_eq!(parse_range("bytes=0-1,5-6", 5000), RangeRequest::Malformed);
    }

    #[tokio::test]
    async fn test_stream_partial_content() {
        let (state, dir) = test_state();
        let track_id = seed_local_file(&state, dir.path(), &[0u8; 1000]);

        let app = build_test_router(state);
        let (status, headers, body) = get_with_range(
            app,
            &format!("/rest/stream?{}&id={}", auth_query(""), track_id),
            "bytes=100-199",
        )
        .await;

        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(headers[header::CONTENT_RANGE], "bytes 100-199/1000");
        assert_eq!(body.len(), 100);
    }

    #[tokio::test]
    async fn test_stream_unsatisfiable_range_is_416() {
        let (state, dir) = test_state();
        let track_id = seed_local_file(&state, dir.path(), &[0u8; 1000]);

        let app = build_test_router(state);
        let (status, headers, body) = get_with_range(
            app,
            &format!("/rest/stream?{}&id={}", auth_query(""), track_id),
            "bytes=5000-6000",
        )
        .await;

        assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(headers[header::CONTENT_RANGE], "bytes */1000");
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn test_stream_malformed_range_serves_whole_body() {
        let (state, dir) = test_state();
        let track_id = seed_local_file(&state, dir.path(), &[0u8; 1000]);

        let app = build_test_router(state);
        let (status, _, body) = get_with_range(
            app,
            &format!("/rest/stream?{}&id={}", auth_query(""), track_id),
            "seconds=0-10",
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.len(), 1000);
    }

    #[tokio::test]
    async fn test_download_serves_the_file() {
        let (state, dir) = test_state();
        let track_id = seed_local_file(&state, dir.path(), &[7u8; 1000]);

        let app = build_test_router(state);
        let (status, _, body) = get_with_range(
            app,
            &format!("/rest/download?{}&id={}", auth_query(""), track_id),
            "bytes=0-99",
        )
        .await;

        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(body, vec![7u8; 100]);
    }
}
