//! Request parameters: the common ones, each endpoint's, and form posts
//! (OpenSubsonic `formPost`).

use super::*;

#[derive(Debug, Default, Deserialize)]
pub(super) struct SubsonicParams {
    pub(super) u: Option<String>,
    pub(super) t: Option<String>,
    pub(super) s: Option<String>,
    pub(super) p: Option<String>,
    #[serde(rename = "apiKey")]
    pub(super) api_key: Option<String>,
    pub(super) f: Option<String>,
    /// A second factor beside `p`: set by `koanSignIn` alone, never read
    /// from another request's parameters.
    #[serde(skip)]
    pub(super) code: Option<String>,
}

impl SubsonicParams {
    pub(super) fn wants_json(&self) -> bool {
        self.f.as_deref() == Some("json")
    }
}

/// A `formPost` body, read by [`form_post`] and handed to the parameter
/// extractors beside the URL's query string, which is left as it came. Not
/// written into the URI: `http` refuses one past 64 KB, and a form is how a
/// client sends what does not fit in a URL.
#[derive(Clone)]
pub(super) struct FormBody(pub(super) Arc<str>);

/// The URL's query string, then the form body's, as one: what handlers read,
/// and what the sign-in throttle must read too, or credentials sent in a form
/// would pass it unseen.
pub(super) fn request_params(
    uri: &axum::http::Uri,
    extensions: &axum::http::Extensions,
) -> Option<String> {
    let query = uri.query().filter(|q| !q.is_empty());
    let form = extensions
        .get::<FormBody>()
        .map(|f| &*f.0)
        .filter(|f| !f.is_empty());
    match (query, form) {
        (Some(q), Some(f)) => Some(format!("{q}&{f}")),
        (q, f) => q.or(f).map(str::to_owned),
    }
}

/// The request's parameters as one query string: the URL's first, then a
/// `formPost` body's, so a parameter in the URL wins where a handler reads one
/// value. In place of axum's `RawQuery`, which sees only the URL.
pub(super) struct RawQuery(pub(super) Option<String>);

impl<S: Send + Sync> FromRequestParts<S> for RawQuery {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(request_params(&parts.uri, &parts.extensions)))
    }
}

/// axum's `Query` over the parameters [`RawQuery`] sees.
pub(super) struct Query<T>(pub(super) T);

impl<T: serde::de::DeserializeOwned, S: Send + Sync> FromRequestParts<S> for Query<T> {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        serde_urlencoded::from_str(
            request_params(&parts.uri, &parts.extensions)
                .as_deref()
                .unwrap_or_default(),
        )
        .map(Query)
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("Failed to deserialize query string: {e}"),
            )
                .into_response()
        })
    }
}

/// Query parameters kept as an ordered list of pairs.
///
/// `serde_urlencoded`, which axum's `Query` uses, cannot deserialise a repeated
/// key into a `Vec`. `createPlaylist` repeats `songId` once per track, which
/// `Query` would reject with a bare HTTP 400 before the handler ran.
pub(super) struct RawParams(pub(super) Vec<(String, String)>);

impl RawParams {
    pub(super) fn parse(query: Option<&str>) -> Self {
        Self(
            form_urlencoded::parse(query.unwrap_or_default().as_bytes())
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect(),
        )
    }

    pub(super) fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Every value for `key`. Clients spell repeated parameters either
    /// `songId=1&songId=2` or `songId[]=1&songId[]=2`; both are accepted.
    pub(super) fn all<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a str> {
        let bracketed = format!("{}[]", key);
        self.0
            .iter()
            .filter(move |(k, _)| k == key || *k == bracketed)
            .map(|(_, v)| v.as_str())
    }

    pub(super) fn auth(&self) -> SubsonicParams {
        SubsonicParams {
            u: self.get("u").map(String::from),
            t: self.get("t").map(String::from),
            s: self.get("s").map(String::from),
            p: self.get("p").map(String::from),
            api_key: self.get("apiKey").map(String::from),
            f: self.get("f").map(String::from),
            code: None,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct IdParam {
    pub(super) id: Option<String>,
    #[serde(flatten)]
    pub(super) auth: SubsonicParams,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AlbumListParams {
    #[serde(rename = "type")]
    pub(super) list_type: Option<String>,
    pub(super) size: Option<i64>,
    pub(super) offset: Option<i64>,
    pub(super) genre: Option<String>,
    pub(super) from_year: Option<i32>,
    pub(super) to_year: Option<i32>,
    #[serde(flatten)]
    pub(super) auth: SubsonicParams,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Search3Params {
    #[serde(flatten)]
    pub(super) auth: SubsonicParams,
    pub(super) query: Option<String>,
    pub(super) artist_count: Option<u32>,
    pub(super) artist_offset: Option<u32>,
    pub(super) album_count: Option<u32>,
    pub(super) album_offset: Option<u32>,
    pub(super) song_count: Option<u32>,
    pub(super) song_offset: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CoverArtParams {
    #[serde(flatten)]
    pub(super) auth: SubsonicParams,
    pub(super) id: Option<String>,
    pub(super) size: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RandomSongsParams {
    #[serde(flatten)]
    pub(super) auth: SubsonicParams,
    pub(super) size: Option<u32>,
    pub(super) genre: Option<String>,
    pub(super) from_year: Option<i32>,
    pub(super) to_year: Option<i32>,
}

/// OpenSubsonic `formPost`: the parameters of an
/// `application/x-www-form-urlencoded` POST body are read here and kept as a
/// [`FormBody`], which the parameter extractors read after the query string,
/// so every handler sees one set of parameters however they were sent,
/// repeated keys included. A body past [`MAX_FORM_BODY`], or not UTF-8, is
/// refused with a Subsonic error.
pub(super) async fn form_post(req: Request, next: Next) -> Response {
    let is_form = req.method() == Method::POST
        && req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .is_some_and(|v| {
                v.trim()
                    .eq_ignore_ascii_case("application/x-www-form-urlencoded")
            });
    if !is_form {
        return next.run(req).await;
    }

    let (mut parts, body) = req.into_parts();
    let json = RawParams::parse(parts.uri.query()).auth().wants_json();
    let Ok(bytes) = axum::body::to_bytes(body, MAX_FORM_BODY).await else {
        return SubsonicResponse::error(
            json,
            &SubsonicError::generic(format!(
                "The form is larger than {} KB",
                MAX_FORM_BODY >> 10
            )),
        );
    };
    let Ok(form) = std::str::from_utf8(&bytes) else {
        return SubsonicResponse::error(json, &SubsonicError::generic("The form is not UTF-8"));
    };
    parts.extensions.insert(FormBody(form.trim().into()));
    next.run(Request::from_parts(parts, axum::body::Body::empty()))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subsonic::testing::*;

    #[test]
    fn test_raw_params_repeated_keys() {
        let p = RawParams::parse(Some("name=mix&songId=1&songId=2&songId%5B%5D=3"));
        assert_eq!(p.get("name"), Some("mix"));
        assert_eq!(p.all("songId").collect::<Vec<_>>(), vec!["1", "2", "3"]);
    }

    #[tokio::test]
    async fn test_form_post_merges_body_with_query() {
        let (state, dir) = test_state();
        seed_data(&state);
        let second = seed_local_file(&state, dir.path(), b"x");
        let first = {
            let db = Database::open(state.pool.path()).unwrap();
            queries::track_id_by_path(&db.conn, "/music/test.flac")
                .unwrap()
                .unwrap()
        };

        let body = post_form(
            build_test_router(state.clone()),
            "/rest/ping.view",
            &auth_query("f=json"),
        )
        .await;
        assert!(body.contains("\"status\":\"ok\""), "{body}");

        // Auth in the query, the repeated key in the body.
        let body = post_form(
            build_test_router(state.clone()),
            &format!("/rest/createPlaylist?{}", auth_query("f=json")),
            &format!("name=mix&songId={first}&songId%5B%5D={second}"),
        )
        .await;
        let r: serde_json::Value = serde_json::from_str(&body).unwrap();
        let entries = r["subsonic-response"]["playlist"]["entry"]
            .as_array()
            .unwrap();
        let ids: Vec<_> = entries.iter().map(|e| e["id"].clone()).collect();
        assert_eq!(
            ids,
            [
                uid_of(&state, queries::UidKind::Track, first),
                uid_of(&state, queries::UidKind::Track, second)
            ]
        );
    }

    /// A form past the 64 KB `http` allows a URI: what `formPost` is for.
    #[tokio::test]
    async fn a_form_longer_than_any_url_reaches_the_handler() {
        let (state, _dir) = test_state();
        seed_data(&state);
        let track = {
            let db = Database::open(state.pool.path()).unwrap();
            queries::track_id_by_path(&db.conn, "/music/test.flac")
                .unwrap()
                .unwrap()
        };
        let uid = uid_of(&state, queries::UidKind::Track, track);
        let base = serve(state).await;
        let http = reqwest::Client::new();
        let songs = vec![format!("songId={uid}"); 2500].join("&");
        assert!(songs.len() > 65_534);

        let created: serde_json::Value = http
            .post(format!(
                "{base}/rest/createPlaylist?{}",
                auth_query("f=json")
            ))
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(format!("name=long&{songs}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let playlist = &created["subsonic-response"]["playlist"];
        assert_eq!(playlist["songCount"], 2500, "{created}");

        let id = playlist["id"].as_str().unwrap();
        let added = vec![format!("songIdToAdd={uid}"); 2500].join("&");
        let updated: serde_json::Value = http
            .post(format!("{base}/rest/updatePlaylist"))
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(format!("{}&playlistId={id}&{added}", auth_query("f=json")))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(updated["subsonic-response"]["status"], "ok", "{updated}");
        let got: serde_json::Value = http
            .get(format!(
                "{base}/rest/getPlaylist?{}&id={id}",
                auth_query("f=json")
            ))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            got["subsonic-response"]["playlist"]["songCount"], 5000,
            "{got}"
        );
    }

    #[tokio::test]
    async fn a_get_reads_its_query_and_an_oversized_form_is_refused() {
        let (state, _dir) = test_state();
        let base = serve(state).await;
        let http = reqwest::Client::new();
        let ping: serde_json::Value = http
            .get(format!("{base}/rest/ping?{}", auth_query("f=json")))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(ping["subsonic-response"]["status"], "ok", "{ping}");

        let padding = "x".repeat(MAX_FORM_BODY + 1);
        let refused: serde_json::Value = http
            .post(format!("{base}/rest/ping?f=json"))
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(format!("{}&pad={padding}", auth_query("")))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let r = &refused["subsonic-response"];
        assert_eq!(r["status"], "failed", "{refused}");
        assert!(
            r["error"]["message"]
                .as_str()
                .unwrap()
                .contains("larger than"),
            "{refused}"
        );
    }
}
