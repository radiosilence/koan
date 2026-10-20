//! The UI's pages, rendered whole for a page load and as content alone for the
//! UI's own navigation, which keeps the shell (and the player in it) in place.
//!
//! Track rows carry what the player needs in data attributes; the player reads
//! them from the page rather than asking the server again.

use std::collections::HashMap;
use std::sync::Arc;

use askama::Template;
use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use koan_core::auth::Role;
use koan_core::db::queries::batch::CreditsOf;
use koan_core::db::queries::{self, AlbumRow, TrackRow};
use koan_core::helpers::{CreditPart, ShareTarget, credit_parts, credits_by_id};

use super::browse::{self, Browse, Kind};
use super::favourite::Hearts;
use super::{PARTIAL, UiState, events, html, open, patch};
use crate::auth::AuthUser;
use crate::html::{Html, NOTHING, Render};
use crate::share::{blocking, duration, not_found};
use koan_core::shelves::{self, Shelf, Summary};

const GENRES_OFFERED: u32 = 80;

/// Why an action failed, drawn where its result would have gone.
#[derive(Template)]
#[template(path = "failed.html")]
pub(super) struct Failed<'a> {
    pub(super) id: &'a str,
    pub(super) class: &'a str,
    pub(super) message: &'a str,
}

pub(super) const KICKER: &str = "m-0 text-fine text-muted lowercase";
pub(super) const SUB: &str = "mt-0 mb-3.5 text-muted wrap-anywhere";
pub(super) const EMPTY: &str = "text-muted";
pub(super) const ERROR: &str = "text-bad";
const ACTIONS: &str = "flex flex-wrap items-center gap-2";
/// Top-aligned, so the cover's top meets the kicker's first line.
const HERO: &str =
    "mb-6 flex items-start gap-6 max-wide:flex-col max-wide:items-stretch max-wide:gap-4";
const HERO_COVER: &str = "size-[220px] flex-none bg-surface object-cover \
max-wide:aspect-square max-wide:h-auto max-wide:w-full";
const GRID: &str = "grid grid-cols-[repeat(auto-fill,minmax(160px,1fr))] gap-x-4 gap-y-5 \
max-wide:grid-cols-[repeat(auto-fill,minmax(140px,1fr))] max-wide:gap-x-3 max-wide:gap-y-4";
/// A copyable link: the share, the new key, an invite, the MCP address.
pub(super) const COPY_ROW: &str = "mt-3 flex max-w-form gap-2";
pub(super) const COPY_INPUT: &str = "flex-1 text-control";
pub(super) const COPY_ERROR: &str = "mt-3 max-w-form text-control text-bad";
/// The sign-in and consent pages, which stand outside the shell.
pub(super) const SIGNIN_BODY: &str = "pb-0";
pub(super) const SIGNIN_MAIN: &str = "mx-auto max-w-[360px] px-4 py-16";
pub(super) const SIGNIN_TITLE: &str = "text-brand";
pub(super) const SIGNIN_LABEL: &str = "grid gap-1.5 text-meta text-muted";

#[derive(Template)]
#[template(path = "player/buttons.html")]
struct Buttons {
    row: &'static str,
    small: &'static str,
    big: &'static str,
    prev: &'static str,
}

/// The transport buttons: in the bar, or larger on a phone on the queue page.
pub(super) fn buttons(page: bool) -> Html {
    let b = if page {
        Buttons {
            row: "flex flex-wrap items-center gap-2 max-wide:justify-center",
            small: "size-11 *:size-[18px] max-wide:size-12 max-wide:*:size-6",
            big: "size-11 *:size-[18px] max-wide:size-15 max-wide:*:size-6",
            prev: "",
        }
    } else {
        Buttons {
            row: "flex items-center gap-1.5",
            small: "size-11 *:size-[18px]",
            big: "size-11 *:size-[18px]",
            prev: "max-wide:hidden",
        }
    };
    b.html()
}

#[derive(Template)]
#[template(path = "player/scrub.html")]
struct Scrub<'a> {
    extra: &'a str,
    height: &'a str,
}

/// Position and length around the seek bar.
pub(super) fn scrub(extra: &str, height: &str) -> Html {
    Scrub { extra, height }.html()
}

/// `proxied`: an authenticating proxy is trusted, so the page renews its
/// session from the proxy rather than a refresh cookie.
#[derive(Template)]
#[template(path = "shell.html")]
struct Shell<'a> {
    title: &'a str,
    content: &'a Html,
    username: &'a str,
    /// Whether the wide sidebar links the Library page, for its rescan.
    admin: bool,
    auth_enabled: bool,
    proxied: bool,
}

#[derive(Template)]
#[template(path = "page.html")]
struct Page<'a> {
    title: &'a str,
    inner: &'a Html,
}

/// The whole page, or only its content when the UI's script asked for that.
pub(super) fn respond(
    s: &UiState,
    headers: &HeaderMap,
    user: &AuthUser,
    title: &str,
    inner: &Html,
) -> Response {
    let content = Page { title, inner }.html();
    if headers.contains_key(PARTIAL) {
        html(StatusCode::OK, content)
    } else {
        html(
            StatusCode::OK,
            Shell {
                title,
                content: &content,
                username: &user.username,
                admin: super::scan::allowed(user),
                auth_enabled: s.auth_enabled,
                proxied: s.proxy_auth.is_some(),
            }
            .html(),
        )
    }
}

pub(super) fn unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "the library is unavailable",
    )
        .into_response()
}

#[derive(Template)]
#[template(path = "pages/login.html")]
struct Login<'a> {
    title: &'a str,
    next: &'a str,
    error: Option<&'a str>,
}

pub(super) fn login(next: &str, error: Option<&str>) -> Html {
    Login {
        title: "Sign in",
        next,
        error,
    }
    .html()
}

/// The setup page: the first admin's name and password, chosen here.
#[derive(Template)]
#[template(path = "pages/setup.html")]
struct Setup<'a> {
    title: &'a str,
    username: &'a str,
    error: Option<&'a str>,
}

pub(super) fn setup(username: &str, error: Option<&str>) -> Html {
    Setup {
        title: "Set up",
        username,
        error,
    }
    .html()
}

fn year(date: Option<&str>) -> &str {
    date.and_then(|d| d.get(..4)).unwrap_or("")
}

/// Each album's cover version: when its files last changed. A cover URL
/// carries it, so the URL changes whenever the art might have and the browser
/// can keep each one for good. One query for a page of albums.
pub(super) type Versions = HashMap<i64, i64>;

fn cover_versions(conn: &rusqlite::Connection, album_ids: &[i64]) -> Versions {
    if album_ids.is_empty() {
        return Versions::new();
    }
    let sql = format!(
        "SELECT album_id, MAX(COALESCE(mtime, 0)) FROM tracks WHERE album_id IN ({}) GROUP BY album_id",
        vec!["?"; album_ids.len()].join(",")
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Versions::new();
    };
    stmt.query_map(rusqlite::params_from_iter(album_ids), |r| {
        Ok((r.get(0)?, r.get(1)?))
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

pub(super) fn cover_url(album_id: i64, size: u32, versions: &Versions) -> String {
    format!(
        "/ui/cover/{album_id}?size={size}&v={}",
        versions.get(&album_id).copied().unwrap_or(0)
    )
}

fn album_versions(conn: &rusqlite::Connection, albums: &[AlbumRow]) -> Versions {
    cover_versions(conn, &albums.iter().map(|a| a.id).collect::<Vec<_>>())
}

pub(super) fn track_versions(conn: &rusqlite::Connection, tracks: &[TrackRow]) -> Versions {
    let mut ids: Vec<i64> = tracks.iter().filter_map(|t| t.album_id).collect();
    ids.sort_unstable();
    ids.dedup();
    cover_versions(conn, &ids)
}

struct Tile<'a> {
    album: &'a AlbumRow,
    src: String,
    artist: Html,
    heart: Html,
}

#[derive(Template)]
#[template(path = "library/tiles.html")]
struct Tiles<'a> {
    tiles: Vec<Tile<'a>>,
}

/// Record tiles, each with its heart over the corner where the account may
/// favourite.
fn cells(
    albums: &[AlbumRow],
    versions: &Versions,
    credits: &Credits,
    hearts: Option<&Hearts>,
) -> Html {
    let tiles = albums
        .iter()
        .map(|a| Tile {
            album: a,
            src: cover_url(a.id, crate::covers::GRID, versions),
            artist: credit_links(
                &credits.album(Some(a.id), &a.artist_name, Some(a.artist_id)),
                true,
            ),
            heart: hearts.map(|h| h.tile(a.id)).unwrap_or_default(),
        })
        .collect();
    Tiles { tiles }.html()
}

/// The codecs and genres the filters offer.
pub(super) type Options = Arc<(Vec<String>, Vec<String>)>;

/// How long the filter options are reused. Counting genres reads every track,
/// and a library changes far more slowly than people page through it.
const OPTIONS_TTL: std::time::Duration = std::time::Duration::from_secs(300);

fn filter_options(s: &UiState) -> Option<Options> {
    let mut held = s.options.lock().ok()?;
    if let Some((at, options)) = held.as_ref()
        && at.elapsed() < OPTIONS_TTL
    {
        return Some(options.clone());
    }
    let db = open(&s.pool)?;
    let options = Arc::new((
        queries::album_codecs(&db.conn).unwrap_or_default(),
        queries::genres(&db.conn, GENRES_OFFERED).unwrap_or_default(),
    ));
    *held = Some((std::time::Instant::now(), options.clone()));
    Some(options)
}

#[derive(Template)]
#[template(path = "library/link.html")]
struct Link<'a> {
    kind: &'a str,
    id: Option<i64>,
    name: &'a str,
}

/// A name linked to its artist or album page (`kind`), or the name alone
/// where the row has no id for it.
pub(super) fn link(kind: &str, id: Option<i64>, name: &str) -> Html {
    Link { kind, id, name }.html()
}

/// The artists credited on a page's tracks and albums, read once for the
/// page. Where the read fails each credit is one link, to the artist it is
/// filed under.
#[derive(Default)]
pub(super) struct Credits {
    tracks: HashMap<i64, Vec<(i64, String)>>,
    albums: HashMap<i64, Vec<(i64, String)>>,
}

impl Credits {
    pub(super) fn load(conn: &rusqlite::Connection, tracks: &[TrackRow], albums: &[i64]) -> Self {
        let read = |of, ids: &[i64]| credits_by_id(conn, of, ids).unwrap_or_default();
        let ids: Vec<i64> = tracks.iter().map(|t| t.id).collect();
        Self {
            tracks: read(CreditsOf::Tracks, &ids),
            albums: read(CreditsOf::Albums, albums),
        }
    }

    /// A track's artist credit.
    pub(super) fn track(&self, t: &TrackRow) -> Vec<CreditPart> {
        let artists = self.tracks.get(&t.id).map(Vec::as_slice).unwrap_or(&[]);
        credit_parts(&t.artist_name, t.artist_id, artists)
    }

    /// An album's artist credit, `credit` as the row names it.
    pub(super) fn album(
        &self,
        id: Option<i64>,
        credit: &str,
        filed: Option<i64>,
    ) -> Vec<CreditPart> {
        let artists = id
            .and_then(|id| self.albums.get(&id))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        credit_parts(credit, filed, artists)
    }
}

#[derive(Template)]
#[template(path = "library/credit.html")]
struct Credit<'a> {
    parts: &'a [CreditPart],
    quiet: bool,
}

/// A credit as it reads, each credited artist's name a link to their page.
pub(super) fn credit_links(parts: &[CreditPart], quiet: bool) -> Html {
    Credit { parts, quiet }.html()
}

#[derive(Template)]
#[template(source = " data-credit=\"{{ json }}\"", ext = "html")]
struct CreditData {
    json: String,
}

/// A row's credit for the player, which draws the queue and Now Playing from
/// the row's data: `[text, artist id or 0]` for each part. Nothing where the
/// credit is one name, which `data-artist` and `data-artist-id` already say.
pub(super) fn credit_data(parts: &[CreditPart]) -> Html {
    if parts.len() < 2 {
        return Html::default();
    }
    let parts: Vec<_> = parts
        .iter()
        .map(|p| serde_json::json!([p.text, p.artist_id.unwrap_or(0)]))
        .collect();
    CreditData {
        json: serde_json::Value::Array(parts).to_string(),
    }
    .html()
}

#[derive(Template)]
#[template(path = "library/credit_line.html")]
struct CreditLine {
    artist: Html,
    album: Option<Html>,
}

/// A track row's subtitle: its artist credit, where given, and its album,
/// where asked for.
pub(super) fn credit_line(t: &TrackRow, artist: Option<&[CreditPart]>, album: bool) -> Html {
    CreditLine {
        artist: artist
            .map(|parts| credit_links(parts, false))
            .unwrap_or_default(),
        album: (album && !t.album_title.is_empty())
            .then(|| link("album", t.album_id, &t.album_title)),
    }
    .html()
}

/// What a page read once for all its rows: covers, credits and hearts.
pub(super) struct RowData<'a> {
    pub versions: &'a Versions,
    pub credits: &'a Credits,
    pub hearts: Option<&'a Hearts>,
}

#[derive(Template)]
#[template(path = "library/track_row.html")]
struct TrackRowView<'a> {
    row: TrackData<'a>,
    n: usize,
    /// Share this track: its album, cued to it. Only where the page has a
    /// `#share-result` to show the link in.
    share: bool,
}

/// What every track row carries, in the library and in history: the track,
/// and what the player and the track menu read from its data attributes
/// (`library::track_open`) and its actions (`library::track_close`).
pub(super) struct TrackData<'a> {
    pub(super) t: &'a TrackRow,
    pub(super) cover: String,
    pub(super) credit: Html,
    /// The credit and record under the title.
    pub(super) sub: Html,
    pub(super) heart: Html,
    pub(super) mark: &'a Html,
}

impl<'a> TrackData<'a> {
    /// `artist` and `album` choose what the line under the title names.
    pub(super) fn new(
        t: &'a TrackRow,
        versions: &Versions,
        credits: &Credits,
        hearts: Option<&'a Hearts>,
        artist: bool,
        album: bool,
    ) -> Self {
        let parts = credits.track(t);
        Self {
            t,
            cover: t
                .album_id
                .map(|a| cover_url(a, crate::covers::LARGE, versions))
                .unwrap_or_default(),
            credit: credit_data(&parts),
            sub: credit_line(t, artist.then_some(&parts[..]), album),
            heart: hearts.map(|h| h.track(t.id)).unwrap_or_default(),
            mark: hearts.map_or(&NOTHING, Hearts::mark),
        }
    }
}

/// A track row. `album` is set where the row stands alone (search) and names
/// the record it comes from.
fn track_row(
    t: &TrackRow,
    n: usize,
    show_artist: bool,
    album: bool,
    share: bool,
    data: &RowData,
) -> Html {
    let RowData {
        versions,
        credits,
        hearts,
    } = *data;
    TrackRowView {
        row: TrackData::new(t, versions, credits, hearts, show_artist, album),
        n,
        share,
    }
    .html()
}

/// Every album `b` lets through, in its order. Whole, as the apps list them:
/// the covers load lazily, so the page costs markup and not images.
fn album_list(
    s: &UiState,
    user: &AuthUser,
    b: &Browse,
) -> Option<(Vec<AlbumRow>, Versions, Credits, Option<Hearts>)> {
    let db = open(&s.pool)?;
    let albums = queries::list_albums(&db.conn, &b.albums(user.user_id, shelves::now())).ok()?;
    let versions = album_versions(&db.conn, &albums);
    let credits = Credits::load(&db.conn, &[], &album_ids(&albums));
    Some((albums, versions, credits, Hearts::load(&db.conn, user)))
}

fn album_ids(albums: &[AlbumRow]) -> Vec<i64> {
    albums.iter().map(|a| a.id).collect()
}

#[derive(Template)]
#[template(path = "pages/albums.html")]
struct AlbumsPage {
    count: usize,
    filtered: bool,
    toolbar: Html,
    tiles: Html,
}

pub(super) async fn albums(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Query(b): Query<Browse>,
    headers: HeaderMap,
) -> Response {
    let b = b.seeded();
    let (st, bb, who) = (s.clone(), b.clone(), user.clone());
    let found = blocking(move || Some((album_list(&st, &who, &bb)?, filter_options(&st)?))).await;
    let Some(((albums, versions, credits, hearts), options)) = found else {
        return unavailable();
    };
    let inner = AlbumsPage {
        count: albums.len(),
        filtered: b.filtered(),
        toolbar: browse::toolbar(&b, Kind::Albums, &options.0, &options.1),
        tiles: cells(&albums, &versions, &credits, hearts.as_ref()),
    }
    .html();
    respond(&s, &headers, &user, "Albums", &inner)
}

/// An album page. `sub` is what follows the credit under the title.
#[derive(Template)]
#[template(path = "pages/album.html")]
struct AlbumPage<'a> {
    album: &'a AlbumRow,
    cover: String,
    credit: Html,
    sub: Vec<String>,
    can_share: bool,
    heart: Html,
    /// Each row, after its disc's heading where the album has several discs.
    rows: Vec<(Option<i32>, Html)>,
}

pub(super) async fn album(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let (st, who) = (s.clone(), user.clone());
    let found = blocking(move || {
        let db = open(&st.pool)?;
        let album = queries::get_album(&db.conn, id).ok()??;
        let tracks = queries::tracks_for_album(&db.conn, id).ok()?;
        let versions = cover_versions(&db.conn, &[id]);
        let credits = Credits::load(&db.conn, &tracks, &[id]);
        Some((
            album,
            tracks,
            versions,
            credits,
            Hearts::load(&db.conn, &who),
        ))
    })
    .await;
    let Some((album, tracks, versions, credits, hearts)) = found else {
        return not_found();
    };
    let can_share = user.role.has_permission(Role::User);
    let discs = tracks
        .iter()
        .map(|t| t.disc.unwrap_or(1))
        .collect::<std::collections::BTreeSet<_>>();
    let data = RowData {
        versions: &versions,
        credits: &credits,
        hearts: hearts.as_ref(),
    };
    let mut disc = None;
    let rows = tracks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let d = t.disc.unwrap_or(1);
            let heading = (discs.len() > 1 && disc != Some(d)).then_some(d);
            disc = Some(d);
            let n = t.track_number.map_or(i + 1, |n| n as usize);
            let row = track_row(
                t,
                n,
                t.artist_name != album.artist_name,
                false,
                can_share,
                &data,
            );
            (heading, row)
        })
        .collect();
    let total: i64 = tracks.iter().filter_map(|t| t.duration_ms).sum();
    let mut sub = Vec::new();
    let y = year(album.date.as_deref());
    if !y.is_empty() {
        sub.push(y.to_owned());
    }
    sub.push(format!(
        "{} track{}",
        tracks.len(),
        if tracks.len() == 1 { "" } else { "s" }
    ));
    sub.push(duration(Some(total)));
    if let Some(codec) = &album.codec {
        sub.push(codec.clone());
    }
    let inner = AlbumPage {
        album: &album,
        cover: cover_url(album.id, crate::covers::LARGE, &versions),
        credit: credit_links(
            &credits.album(Some(album.id), &album.artist_name, Some(album.artist_id)),
            false,
        ),
        sub,
        can_share,
        heart: hearts
            .as_ref()
            .map(|h| h.album(album.id))
            .unwrap_or_default(),
        rows,
    }
    .html();
    respond(&s, &headers, &user, &album.title, &inner)
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
pub(super) struct Cue {
    track: Option<i64>,
}

/// Share the album, or with `?track=` the album cued to that track.
pub(super) async fn share_album(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
    Query(cue): Query<Cue>,
) -> Response {
    let target = ShareTarget::Album {
        album_id: id,
        start_track_id: cue.track,
    };
    share(s, user, target).await
}

pub(super) async fn share_artist(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> Response {
    share(s, user, ShareTarget::Artist(id)).await
}

#[derive(Template)]
#[template(path = "library/share_result.html")]
struct ShareResult {
    result: Result<String, String>,
}

/// Make a share link and show it in the page's `#share-result`.
async fn share(s: UiState, user: AuthUser, target: ShareTarget) -> Response {
    let result = if user.role.has_permission(Role::User) {
        blocking(move || {
            let db = open(&s.pool)?;
            let cfg = koan_core::config::Config::load().unwrap_or_default();
            Some(
                koan_core::helpers::create_native_share(&db, user.user_id, &cfg, &target, None)
                    .map(|o| o.url)
                    .map_err(|e| e.to_string()),
            )
        })
        .await
        .unwrap_or_else(|| Err("The library is unavailable.".into()))
    } else {
        Err("This account cannot make share links.".into())
    };
    let html = ShareResult { result }.html();
    events(vec![patch(&html, None)])
}

#[derive(Template)]
#[template(path = "pages/artists.html")]
struct ArtistsPage {
    artists: Vec<queries::ArtistRow>,
    filtered: bool,
    toolbar: Html,
}

fn artist_rows(s: &UiState, user: i64, b: &Browse) -> Option<Vec<queries::ArtistRow>> {
    let db = open(&s.pool)?;
    queries::list_artists(&db.conn, &b.artists(user, shelves::now())).ok()
}

pub(super) async fn artists(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Query(b): Query<Browse>,
    headers: HeaderMap,
) -> Response {
    let (st, bb, id) = (s.clone(), b.clone(), user.user_id);
    let found = blocking(move || Some((artist_rows(&st, id, &bb)?, filter_options(&st)?))).await;
    let Some((artists, options)) = found else {
        return unavailable();
    };
    let inner = ArtistsPage {
        filtered: b.filtered(),
        toolbar: browse::toolbar(&b, Kind::Artists, &options.0, &options.1),
        artists,
    }
    .html();
    respond(&s, &headers, &user, "Artists", &inner)
}

/// Tracks on one page of the track browser.
const TRACKS_PAGE: u32 = 200;

#[derive(Template)]
#[template(path = "pages/tracks.html")]
struct TracksPage {
    total: u64,
    toolbar: Html,
    rows: Html,
    previous: Option<String>,
    next: Option<String>,
}

/// Every track the filters let through, a page at a time. Picking one plays on
/// through the page.
pub(super) async fn tracks(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Query(b): Query<Browse>,
    headers: HeaderMap,
) -> Response {
    let page = b.page;
    let (st, bb, who) = (s.clone(), b.clone(), user.clone());
    let found = blocking(move || {
        let db = open(&st.pool)?;
        let listing = bb.tracks(who.user_id, shelves::now());
        let total = listing.count(&db.conn).ok()?;
        let rows = listing
            .page(&db.conn, TRACKS_PAGE, page * TRACKS_PAGE)
            .ok()?;
        let versions = track_versions(&db.conn, &rows);
        let credits = Credits::load(&db.conn, &rows, &[]);
        let hearts = Hearts::load(&db.conn, &who);
        drop(db);
        Some((rows, total, versions, credits, hearts, filter_options(&st)?))
    })
    .await;
    let Some((rows, total, versions, credits, hearts, options)) = found else {
        return unavailable();
    };
    let first = (page * TRACKS_PAGE) as usize;
    let data = RowData {
        versions: &versions,
        credits: &credits,
        hearts: hearts.as_ref(),
    };
    let at = |p: u32| {
        let q = b.query();
        let sep = if q.is_empty() { "" } else { "&" };
        format!("/tracks?{q}{sep}page={p}")
    };
    let inner = TracksPage {
        total,
        toolbar: browse::toolbar(&b, Kind::Tracks, &options.0, &options.1),
        rows: rows
            .iter()
            .enumerate()
            .map(|(i, t)| track_row(t, first + i + 1, true, true, false, &data))
            .collect(),
        previous: (page > 0).then(|| at(page - 1)),
        next: ((((page + 1) * TRACKS_PAGE) as u64) < total).then(|| at(page + 1)),
    }
    .html();
    respond(&s, &headers, &user, "Tracks", &inner)
}

/// An artist page. `heart` is there where the account may favourite, and
/// so share.
#[derive(Template)]
#[template(path = "pages/artist.html")]
struct ArtistPage<'a> {
    artist: &'a queries::ArtistRow,
    heart: Option<Html>,
    tiles: Html,
}

pub(super) async fn artist(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let (st, who) = (s.clone(), user.clone());
    let found = blocking(move || {
        let db = open(&st.pool)?;
        let artist = queries::get_artist(&db.conn, id).ok()??;
        let albums = queries::albums_for_artist(&db.conn, id).ok()?;
        let versions = album_versions(&db.conn, &albums);
        let credits = Credits::load(&db.conn, &[], &album_ids(&albums));
        Some((
            artist,
            albums,
            versions,
            credits,
            Hearts::load(&db.conn, &who),
        ))
    })
    .await;
    let Some((artist, albums, versions, credits, hearts)) = found else {
        return not_found();
    };
    let inner = ArtistPage {
        heart: hearts.as_ref().map(|h| h.artist(artist.id)),
        tiles: cells(&albums, &versions, &credits, hearts.as_ref()),
        artist: &artist,
    }
    .html();
    respond(&s, &headers, &user, &artist.name, &inner)
}

#[derive(Template)]
#[template(path = "pages/playlists.html")]
struct PlaylistsPage {
    lists: Vec<queries::PlaylistRow>,
}

pub(super) async fn playlists(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    let st = s.clone();
    let found = blocking(move || {
        let db = open(&st.pool)?;
        queries::list_playlists(&db.conn, user.user_id).ok()
    })
    .await;
    let Some(lists) = found else {
        return unavailable();
    };
    let inner = PlaylistsPage { lists }.html();
    respond(&s, &headers, &user, "Playlists", &inner)
}

#[derive(Template)]
#[template(path = "pages/playlist.html")]
struct PlaylistPage<'a> {
    list: &'a queries::PlaylistRow,
    cover: Option<String>,
    owner: Option<&'a str>,
    comment: Option<&'a str>,
    count: usize,
    rows: Html,
}

/// A playlist, laid out like an album so its rows play and queue the same way.
/// The caller's own playlists and anyone's public ones.
pub(super) async fn playlist(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let (st, who) = (s.clone(), user.clone());
    let found = blocking(move || {
        let db = open(&st.pool)?;
        let me = queries::auth::resolve_user(&db.conn, who.user_id).ok()?;
        let list = queries::get_playlist(&db.conn, id)
            .ok()?
            .filter(|p| p.readable_by(me))?;
        let tracks = queries::playlist_tracks(&db.conn, id).ok()?;
        let versions = track_versions(&db.conn, &tracks);
        let credits = Credits::load(&db.conn, &tracks, &[]);
        Some((
            list,
            tracks,
            versions,
            credits,
            Hearts::load(&db.conn, &who),
        ))
    })
    .await;
    let Some((list, tracks, versions, credits, hearts)) = found else {
        return not_found();
    };
    let data = RowData {
        versions: &versions,
        credits: &credits,
        hearts: hearts.as_ref(),
    };
    let inner = PlaylistPage {
        list: &list,
        cover: tracks
            .iter()
            .find_map(|t| t.album_id)
            .map(|a| cover_url(a, crate::covers::LARGE, &versions)),
        owner: list.owner.as_deref().filter(|o| !o.is_empty()),
        comment: list.comment.as_deref().filter(|c| !c.is_empty()),
        count: tracks.len(),
        rows: tracks
            .iter()
            .enumerate()
            .map(|(i, t)| track_row(t, i + 1, true, true, false, &data))
            .collect(),
    }
    .html();
    respond(&s, &headers, &user, &list.name, &inner)
}

/// What the search's results area shows.
enum Results<'a> {
    Blank,
    Unavailable,
    Nothing(&'a str),
    Shelf(Html),
}

#[derive(Template)]
#[template(path = "library/results.html")]
struct ResultsView<'a> {
    found: Results<'a>,
}

fn results(s: &UiState, user: &AuthUser, q: &str) -> Html {
    let q = q.trim();
    let found = if q.is_empty() {
        Results::Blank
    } else {
        let shelf = Shelf::Search(q);
        let found = open(&s.pool).and_then(|db| {
            let summary =
                shelves::summary(&db.conn, shelf, user.user_id, shelves::now(), false).ok()?;
            let versions = shelf_versions(&db.conn, &summary);
            let credits = shelf_credits(&db.conn, &summary);
            Some((summary, versions, credits, Hearts::load(&db.conn, user)))
        });
        match found {
            None => Results::Unavailable,
            Some((summary, ..)) if summary.is_empty() => Results::Nothing(q),
            Some((summary, versions, credits, hearts)) => Results::Shelf(shelf_sections(
                &summary,
                shelf,
                &RowData {
                    versions: &versions,
                    credits: &credits,
                    hearts: hearts.as_ref(),
                },
            )),
        }
    };
    ResultsView { found }.html()
}

#[derive(Template)]
#[template(path = "pages/search.html")]
struct SearchPage<'a> {
    q: &'a str,
    results: Html,
}

pub(super) async fn search(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let q = params.get("q").cloned().unwrap_or_default();
    let (st, who) = (s.clone(), user.clone());
    let query = q.clone();
    let found = blocking(move || Some(results(&st, &who, &query)))
        .await
        .unwrap_or_default();
    let inner = SearchPage {
        q: &q,
        results: found,
    }
    .html();
    respond(&s, &headers, &user, "Search", &inner)
}

/// Datastar sends its signals as JSON in `datastar`; a plain request sends `q`.
pub(super) async fn search_results(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let q = params
        .get("datastar")
        .and_then(|d| serde_json::from_str::<serde_json::Value>(d).ok())
        .and_then(|v| v.get("q")?.as_str().map(str::to_owned))
        .or_else(|| params.get("q").cloned())
        .unwrap_or_default();
    let html = blocking(move || Some(results(&s, &user, &q)))
        .await
        .unwrap_or_default();
    events(vec![patch(&html, None)])
}

#[derive(Template)]
#[template(path = "pages/library.html")]
struct LibraryPage {
    /// Whether the page has the rescan, which a wide screen shows alone.
    rescan: bool,
    scan: Html,
}

/// The library's own lists, which the sidebar shows as links and a phone, with
/// one tab for them all, as this page.
pub(super) async fn library(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    let inner = LibraryPage {
        rescan: super::scan::allowed(&user),
        scan: super::scan::section(&s, &user),
    }
    .html();
    respond(&s, &headers, &user, "Library", &inner)
}

/// What a shelf page says with nothing on it.
pub(super) struct EmptyShelf {
    pub title: &'static str,
    pub detail: &'static str,
}

/// The covers a shelf's previews show.
fn shelf_versions(conn: &rusqlite::Connection, s: &Summary) -> Versions {
    let mut versions = album_versions(conn, &s.albums.preview);
    versions.extend(track_versions(conn, &s.tracks.preview));
    versions
}

/// The credits a shelf's previews show.
fn shelf_credits(conn: &rusqlite::Connection, s: &Summary) -> Credits {
    Credits::load(conn, &s.tracks.preview, &album_ids(&s.albums.preview))
}

#[derive(Template)]
#[template(path = "library/shelf.html")]
struct ShelfView<'a> {
    /// Each section's total, the browser it links to and its preview, where it
    /// has a preview to show.
    artists: Option<(u64, String, &'a [queries::ArtistRow])>,
    albums: Option<(u64, String, Html)>,
    tracks: Option<(u64, String, Html)>,
}

/// The previews of a shelf.
fn shelf_sections(s: &Summary, shelf: Shelf, data: &RowData) -> Html {
    let browser = |kind| browse::shelf_browser(shelf, kind);
    ShelfView {
        artists: (!s.artists.preview.is_empty()).then(|| {
            (
                s.artists.total,
                browser(Kind::Artists),
                &s.artists.preview[..],
            )
        }),
        albums: (!s.albums.preview.is_empty()).then(|| {
            let tiles = cells(&s.albums.preview, data.versions, data.credits, data.hearts);
            (s.albums.total, browser(Kind::Albums), tiles)
        }),
        tracks: (!s.tracks.preview.is_empty()).then(|| {
            let rows = s
                .tracks
                .preview
                .iter()
                .enumerate()
                .map(|(i, t)| track_row(t, i + 1, true, true, false, data))
                .collect();
            (s.tracks.total, browser(Kind::Tracks), rows)
        }),
    }
    .html()
}

#[derive(Template)]
#[template(path = "pages/shelf.html")]
struct ShelfPage<'a> {
    title: &'a str,
    sections: Option<Html>,
    empty: EmptyShelf,
}

/// A shelf page: its title, then its previews, or what it says when empty.
async fn shelf_page(
    s: UiState,
    user: AuthUser,
    headers: HeaderMap,
    shelf: Shelf<'static>,
    title: &'static str,
    empty: EmptyShelf,
) -> Response {
    let (st, who) = (s.clone(), user.clone());
    let found = blocking(move || {
        let db = open(&st.pool)?;
        let summary = shelves::summary(&db.conn, shelf, who.user_id, shelves::now(), false).ok()?;
        let versions = shelf_versions(&db.conn, &summary);
        let credits = shelf_credits(&db.conn, &summary);
        Some((summary, versions, credits, Hearts::load(&db.conn, &who)))
    })
    .await;
    let Some((summary, versions, credits, hearts)) = found else {
        return unavailable();
    };
    let sections = (!summary.is_empty()).then(|| {
        shelf_sections(
            &summary,
            shelf,
            &RowData {
                versions: &versions,
                credits: &credits,
                hearts: hearts.as_ref(),
            },
        )
    });
    let inner = ShelfPage {
        title,
        sections,
        empty,
    }
    .html();
    respond(&s, &headers, &user, title, &inner)
}

/// The signed-in account's favourite artists, records and tracks.
pub(super) async fn favourites(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    shelf_page(
        s,
        user,
        headers,
        Shelf::Favourites,
        "Favourites",
        EmptyShelf {
            title: "Nothing favourited yet.",
            detail: "Artists, records and tracks favourited in the kōan apps, or in any Subsonic app \
signed in as you, are listed here.",
        },
    )
    .await
}

/// What the signed-in account played lately, each once and newest first by
/// its latest play: the apps' Recently played.
pub(super) async fn recent(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    shelf_page(
        s,
        user,
        headers,
        Shelf::Recent,
        "Recently played",
        EmptyShelf {
            title: "Nothing played in the last 30 days.",
            detail: "What you play here, in the kōan apps or in a Subsonic app signed in as you, \
is gathered here for a month, each artist, record and track once.",
        },
    )
    .await
}

#[derive(Template)]
#[template(path = "pages/queue.html")]
struct QueuePage;

/// The queue lives in the browser, so the page is a frame the script fills.
pub(super) async fn queue(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    let inner = QueuePage.html();
    respond(&s, &headers, &user, "Queue", &inner)
}
