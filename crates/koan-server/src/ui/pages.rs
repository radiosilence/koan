//! The UI's pages, rendered whole for a page load and as content alone for the
//! UI's own navigation, which keeps the shell (and the player in it) in place.
//!
//! Track rows carry what the player needs in data attributes; the player reads
//! them from the page rather than asking the server again.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use koan_core::auth::Role;
use koan_core::db::queries::{self, AlbumRow, TrackRow};
use koan_core::helpers::ShareTarget;

use super::browse::{self, Browse, Kind};
use super::favourite::Hearts;
use super::{PARTIAL, UiState, events, html, open, patch};
use crate::auth::AuthUser;
use crate::share::{blocking, duration, escape, not_found};
use koan_core::shelves::{self, Shelf, Summary};

const GENRES_OFFERED: u32 = 80;

const ICON_PREV: &str =
    "<svg viewBox=\"0 0 24 24\" aria-hidden=true><path d=\"M6 5h2v14H6zM20 5v14L9 12z\"/></svg>";
const ICON_NEXT: &str =
    "<svg viewBox=\"0 0 24 24\" aria-hidden=true><path d=\"M16 5h2v14h-2zM4 5v14l11-7z\"/></svg>";
const ICON_CHEVRON: &str = "<svg class=\"size-[0.8em] fill-none stroke-current stroke-[2.5]\" \
viewBox=\"0 0 24 24\" aria-hidden=true><path d=\"M9 5l7 7-7 7\"/></svg>";
// `playing` is on the body while music plays.
const ICON_PLAY: &str = "<svg class=\"in-[.playing]:hidden\" viewBox=\"0 0 24 24\" aria-hidden=true>\
     <path d=\"M7 4v16l13-8z\"/></svg>";
const ICON_PAUSE: &str = "<svg class=\"hidden in-[.playing]:inline\" viewBox=\"0 0 24 24\" aria-hidden=true>\
     <path d=\"M6 4h4v16H6zM14 4h4v16h-4z\"/></svg>";

/// A track row's actions on a narrow screen: one button opening the row's
/// menu, which right-click and long-press open on any screen. The row's own
/// buttons stand beside it on a wide screen only.
pub(super) const MORE: &str = "<button class=\"quiet wide:hidden\" data-act=menu aria-haspopup=menu \
aria-label=\"More\" title=\"More\">⋯</button>";

/// The menu every track row opens, filled for the row by the UI's script.
/// Beside the pointer on a wide screen, a sheet above the bar on a phone.
const TRACK_MENU: &str = "<div id=track-menu popover=manual role=menu class=\"fixed inset-auto m-0 hidden min-w-52 \
flex-col open:flex border border-rule bg-bg p-1 text-ink max-wide:inset-x-3 max-wide:w-auto \
max-wide:bottom-[calc(var(--bar-h)+var(--tabs-h)+env(safe-area-inset-bottom)+8px)]\" aria-label=Track></div>";

pub(super) const KICKER: &str = "m-0 text-fine text-muted lowercase";
pub(super) const SUB: &str = "mt-0 mb-3.5 text-muted wrap-anywhere";
pub(super) const EMPTY: &str = "text-muted";
pub(super) const ERROR: &str = "text-bad";
const ACTIONS: &str = "flex flex-wrap items-center gap-2";
/// A heading with its buttons, above a track list. The row keeps the space a
/// heading would, above and below, since buttons beside it are taller than it
/// and would otherwise sit on the list.
pub(super) const LIST_HEAD: &str =
    "mt-6 mb-3 flex flex-wrap items-center justify-between gap-x-3 gap-y-2 [&>h1]:my-0 [&>h2]:my-0";
/// Top-aligned, so the cover's top meets the kicker's first line.
const HERO: &str =
    "mb-6 flex items-start gap-6 max-wide:flex-col max-wide:items-stretch max-wide:gap-4";
const HERO_COVER: &str = "size-[220px] flex-none bg-surface object-cover \
max-wide:aspect-square max-wide:h-auto max-wide:w-full";
const GRID: &str = "grid grid-cols-[repeat(auto-fill,minmax(160px,1fr))] gap-x-4 gap-y-5 \
max-wide:grid-cols-[repeat(auto-fill,minmax(140px,1fr))] max-wide:gap-x-3 max-wide:gap-y-4";
/// A row of the artist and playlist lists: the link, its name and its count.
/// No rules between rows, as in every list of the theme.
const LIST_ROW: &str = "flex min-w-0 items-baseline gap-3 px-2 py-2.5 text-ink hover:bg-hover/30 \
hover:no-underline max-wide:px-0";
const LIST_NAME: &str = "min-w-0 flex-1 truncate";
const LIST_COUNT: &str = "text-meta text-muted";
/// A copyable link: the share, the new key, an invite, the MCP address.
pub(super) const COPY_ROW: &str = "mt-3 flex max-w-form gap-2";
pub(super) const COPY_INPUT: &str = "flex-1 text-control";
pub(super) const COPY_ERROR: &str = "mt-3 max-w-form text-control text-bad";
/// The sign-in and consent pages, which stand outside the shell.
pub(super) const SIGNIN_BODY: &str = "pb-0";
pub(super) const SIGNIN_MAIN: &str = "mx-auto max-w-[360px] px-4 py-16";
pub(super) const SIGNIN_TITLE: &str = "text-brand";

/// The transport buttons: in the bar, or larger on a phone on the queue page.
fn buttons(page: bool) -> String {
    let icon = "inline-flex items-center justify-center p-0 text-ink hover:bg-transparent hover:text-strong \
*:fill-current";
    let (row, small, big, prev) = if page {
        (
            "flex flex-wrap items-center gap-2 max-wide:justify-center",
            "size-11 *:size-[18px] max-wide:size-12 max-wide:*:size-6",
            "size-11 *:size-[18px] max-wide:size-15 max-wide:*:size-6",
            "",
        )
    } else {
        (
            "flex items-center gap-1.5",
            "size-11 *:size-[18px]",
            "size-11 *:size-[18px]",
            "max-wide:hidden",
        )
    };
    format!(
        "<div class=\"{row}\"><button class=\"{icon} {small} {prev} border-transparent\" data-ctl=prev aria-label=Previous>{ICON_PREV}</button>\
<button class=\"{icon} {big} border-ink\" data-ctl=play aria-label=\"Play or pause\">{ICON_PLAY}{ICON_PAUSE}</button>\
<button class=\"{icon} {small} border-transparent\" data-ctl=next aria-label=Next>{ICON_NEXT}</button></div>"
    )
}

/// Position and length around the seek bar. `extra` places it; `height` is the
/// slider's hit area, which the transport bar has no room to make 44 px.
fn scrub(extra: &str, height: &str) -> String {
    format!(
        "<div class=\"flex w-full items-center gap-2.5 text-fine text-muted tabular-nums {extra}\">\
<span data-np=pos>0:00</span>\
<input type=range class=\"{height} min-w-0 flex-1\" data-ctl=seek min=0 max=0 step=0.1 value=0 aria-label=Position>\
<span data-np=len>0:00</span></div>"
    )
}

pub(super) fn head(title: &str) -> String {
    format!(
        "<!doctype html><html lang=en><head><meta charset=utf-8>\
<meta name=viewport content=\"width=device-width,initial-scale=1,viewport-fit=cover\">\
<meta name=color-scheme content=\"dark light\">\
<meta name=theme-color content=\"#1e1e1e\" media=\"(prefers-color-scheme: dark)\">\
<meta name=theme-color content=\"#ffffff\" media=\"(prefers-color-scheme: light)\"><meta name=robots content=\"noindex,nofollow\">\
<title>{} · kōan</title>{}<link rel=stylesheet href=\"{}\">",
        escape(title),
        crate::share::icon_links("/ui/assets"),
        super::ASSETS.css,
    )
}

/// The sidebar's links, which become the tab bar on a phone: six tabs, each as
/// wide as its label, which fit a 360 px screen at the meta size. A phone has
/// no room for a seventh, so there Playlists becomes Library, a page of the
/// library's own lists; the sidebar lists them directly.
///
/// The theme's navigation row: lowercase in `muted`, the page shown in the
/// accent with a rule on its leading edge, and no fill. On a phone, the tab
/// bar's: the page shown in the accent and underlined.
const NAV_LINK: &str = "border-l-2 border-transparent px-2.5 py-1.5 text-muted lowercase hover:text-ink \
hover:no-underline aria-[current=page]:border-brand aria-[current=page]:text-brand max-wide:flex \
max-wide:flex-auto max-wide:items-center max-wide:justify-center max-wide:border-l-0 max-wide:px-0 \
max-wide:text-meta max-wide:underline-offset-4 max-wide:aria-[current=page]:underline";

/// The account's links in the sidebar.
const ACCOUNT_LINK: &str = "px-1.5 py-1 text-meta whitespace-nowrap text-muted lowercase hover:text-ink \
hover:no-underline aria-[current=page]:text-brand";

/// `proxied`: an authenticating proxy is trusted, so the page renews its
/// session from the proxy rather than a refresh cookie.
fn shell(title: &str, content: &str, user: &AuthUser, auth_enabled: bool, proxied: bool) -> String {
    // On a wide screen the sidebar ends with who is signed in and the version;
    // the account page has everything else. A phone has an Account tab instead.
    let signed_in = if auth_enabled {
        format!(
            "<form class=\"flex min-w-0 flex-wrap items-center gap-x-1.5 gap-y-0.5 px-1 text-meta text-muted\" \
method=post action=\"/auth/signout\"><span class=\"min-w-0 flex-[1_0_100%] truncate px-1.5 pb-0.5 text-ink\">{}</span>\
<a class=\"{ACCOUNT_LINK}\" href=\"/account\" data-nav=account>Account</a>\
<button class=\"quiet px-1.5 py-1 text-meta\">Sign out</button></form>",
            escape(&user.username)
        )
    } else {
        format!(
            "<a class=\"{ACCOUNT_LINK} self-start\" href=\"/account\" data-nav=account>Account</a>"
        )
    };
    let account = format!(
        "<div class=\"mt-auto flex min-w-0 flex-col gap-1 max-wide:hidden\">{signed_in}\
<a class=\"px-1.5 py-0.5 text-fine text-muted tabular-nums hover:text-ink hover:no-underline\" \
href=\"https://github.com/radiosilence/koan/releases/tag/v{v}\">kōan {v}</a></div>",
        v = env!("CARGO_PKG_VERSION")
    );
    format!(
        "{head}<script type=module src=\"{datastar}\"></script>\
<script src=\"{player}\" defer></script><script src=\"{ui}\" defer></script>\
</head><body{proxied}><nav class=\"fixed top-0 bottom-(--bar-h) left-0 z-4 flex w-(--side-w) flex-col gap-0.5 border-r \
border-rule bg-bg px-2.5 py-4 wide:overflow-y-auto pt-[max(16px,env(safe-area-inset-top))] max-wide:top-auto max-wide:right-0 \
max-wide:bottom-0 max-wide:h-[calc(var(--tabs-h)+env(safe-area-inset-bottom))] max-wide:w-auto \
max-wide:flex-row max-wide:gap-0 max-wide:border-t max-wide:border-r-0 max-wide:p-0 \
max-wide:pb-[env(safe-area-inset-bottom)]\" aria-label=Library>\
<a class=\"mb-3 px-3 py-2 text-[22px] font-extralight text-brand hover:text-ink hover:no-underline \
max-wide:hidden\" href=\"/\">kōan</a>\
<a class=\"{NAV_LINK}\" href=\"/albums\" data-nav=albums>Albums</a>\
<a class=\"{NAV_LINK}\" href=\"/artists\" data-nav=artists>Artists</a>\
<a class=\"{NAV_LINK} max-wide:hidden\" href=\"/tracks\" data-nav=tracks>Tracks</a>\
<a class=\"{NAV_LINK} max-wide:hidden\" href=\"/playlists\" data-nav=playlists>Playlists</a>\
<a class=\"{NAV_LINK} wide:hidden\" href=\"/library\" data-nav=\"library playlists recent favourites history tracks\">Library</a>\
<a class=\"{NAV_LINK}\" href=\"/search\" data-nav=search>Search</a>\
<a class=\"{NAV_LINK}\" href=\"/queue\" data-nav=queue>Queue</a>\
<a class=\"{NAV_LINK} mt-3 max-wide:hidden\" href=\"/recent\" data-nav=recent>Recently played</a>\
<a class=\"{NAV_LINK} max-wide:hidden\" href=\"/favourites\" data-nav=favourites>Favourites</a>\
<a class=\"{NAV_LINK} max-wide:hidden\" href=\"/history\" data-nav=history>History</a>\
<a class=\"{NAV_LINK} wide:hidden\" href=\"/account\" data-nav=account>Account</a>{account}</nav>\
<main id=content class=\"ml-(--side-w) min-w-0 px-7 \
pt-[max(24px,env(safe-area-inset-top))] pb-10 max-wide:ml-0 max-wide:p-4 \
max-wide:pt-[max(16px,env(safe-area-inset-top))]\">{content}</main>\
\
<footer class=\"fixed inset-x-0 bottom-(--tabs-h) z-5 grid h-[calc(var(--bar-h)+env(safe-area-inset-bottom))] \
grid-cols-[minmax(0,1fr)_minmax(0,2fr)_minmax(0,1fr)] items-center gap-4 border-t border-rule bg-bg px-4 \
pb-[env(safe-area-inset-bottom)] max-wide:bottom-[calc(var(--tabs-h)+env(safe-area-inset-bottom))] \
max-wide:h-(--bar-h) max-wide:grid-cols-[minmax(0,1fr)_auto] max-wide:gap-2 max-wide:pr-2 max-wide:pb-0 \
max-wide:pl-3\">\
<progress class=\"absolute inset-x-0 -top-px hidden h-0.5 w-full appearance-none border-0 bg-rule \
max-wide:block [&::-moz-progress-bar]:bg-brand [&::-webkit-progress-bar]:bg-rule \
[&::-webkit-progress-value]:bg-brand\" data-np=progress max=1 value=0></progress>\
<a class=\"flex min-w-0 items-center gap-2.5 text-ink hover:no-underline\" href=\"/queue\">\
<img class=\"size-10 flex-none bg-surface object-cover\" data-np=cover alt=\"\" hidden>\
<span class=\"flex min-w-0 flex-col\"><span class=\"truncate text-meta text-strong\" data-np=title>Nothing playing</span>\
<span class=\"truncate text-fine text-muted\" data-np=artist></span></span></a>\
<div class=\"flex min-w-0 flex-col items-center gap-1\">{buttons}{scrub}</div></footer>{TRACK_MENU}</body></html>",
        head = head(title),
        datastar = super::ASSETS.datastar_js,
        player = super::ASSETS.player_js,
        ui = super::ASSETS.ui_js,
        buttons = buttons(false),
        scrub = scrub("max-wide:hidden", "h-4"),
        proxied = if proxied { " data-proxied" } else { "" },
    )
}

/// The whole page, or only its content when the UI's script asked for that.
pub(super) fn respond(
    s: &UiState,
    headers: &HeaderMap,
    user: &AuthUser,
    title: &str,
    inner: &str,
) -> Response {
    let content = format!(
        "<section class=\"page\" data-title=\"{}\">{inner}</section>",
        escape(title)
    );
    if headers.contains_key(PARTIAL) {
        html(StatusCode::OK, content)
    } else {
        html(
            StatusCode::OK,
            shell(
                title,
                &content,
                user,
                s.auth_enabled,
                s.proxy_auth.is_some(),
            ),
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

pub(super) fn login(next: &str, error: Option<&str>) -> String {
    let error = error
        .map(|e| format!("<p class=\"m-0 {ERROR}\" role=alert>{}</p>", escape(e)))
        .unwrap_or_default();
    let label = "grid gap-1.5 text-meta text-muted";
    format!(
        "{head}</head><body class=\"{SIGNIN_BODY}\"><main class=\"{SIGNIN_MAIN}\"><h1 class=\"{SIGNIN_TITLE}\">kōan</h1>\
<form class=\"grid gap-3.5\" method=post action=\"/login\"><input type=hidden name=next value=\"{next}\">\
<label class=\"{label}\">Username<input class=\"text-input\" name=username autocomplete=username autocapitalize=none \
spellcheck=false required autofocus></label>\
<label class=\"{label}\">Password<input class=\"text-input\" name=password type=password \
autocomplete=current-password required></label>\
{error}<button class=\"primary\">Sign in</button></form></main></body></html>",
        head = head("Sign in"),
        next = escape(next),
    )
}

/// The setup page: the first admin's name and password, chosen here.
pub(super) fn setup(username: &str, error: Option<&str>) -> String {
    let error = error
        .map(|e| format!("<p class=\"m-0 {ERROR}\" role=alert>{}</p>", escape(e)))
        .unwrap_or_default();
    let label = "grid gap-1.5 text-meta text-muted";
    format!(
        "{head}</head><body class=\"{SIGNIN_BODY}\"><main class=\"{SIGNIN_MAIN}\"><h1 class=\"{SIGNIN_TITLE}\">kōan</h1>\
<p class=\"text-meta text-muted\">This server has no accounts yet. Choose the admin's name and password; \
this page closes once it exists.</p>\
<form class=\"grid gap-3.5\" method=post action=\"/setup\">\
<label class=\"{label}\">Username<input class=\"text-input\" name=username value=\"{username}\" autocomplete=username \
autocapitalize=none spellcheck=false required autofocus></label>\
<label class=\"{label}\">Password<input class=\"text-input\" name=password type=password minlength=8 \
autocomplete=new-password required></label>\
<label class=\"{label}\">Confirm password<input class=\"text-input\" name=confirm type=password minlength=8 \
autocomplete=new-password required></label>\
{error}<button class=\"primary\">Create admin</button></form></main></body></html>",
        head = head("Set up"),
        username = escape(username),
    )
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

/// Record tiles, each with its heart over the corner where the account may
/// favourite.
fn cells(albums: &[AlbumRow], versions: &Versions, hearts: Option<&Hearts>) -> String {
    albums.iter().fold(String::new(), |mut out, a| {
        let _ = write!(
            out,
            "<div class=\"relative min-w-0\"><a class=\"group flex min-w-0 flex-col gap-0.5 text-ink hover:no-underline\" href=\"/album/{id}\">\
<img class=\"mb-1.5 aspect-square h-auto w-full bg-surface object-cover \
[&.missing]:visible [&.missing]:text-transparent\" loading=lazy decoding=async \
width={size} height={size} src=\"{src}\" alt=\"\">\
<span class=\"truncate group-hover:text-strong\">{title}</span><span class=\"truncate text-meta text-muted\">{artist}</span></a>{heart}</div>",
            id = a.id,
            heart = hearts.map(|h| h.tile(a.id)).unwrap_or_default(),
            size = crate::covers::GRID,
            src = cover_url(a.id, crate::covers::GRID, versions),
            title = escape(&a.title),
            artist = escape(&a.artist_name),
        );
        out
    })
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

const ICON_SHARE: &str = "<svg viewBox=\"0 0 24 24\" aria-hidden=true>\
     <path d=\"M12 3l4.5 4.5h-3.5v7h-2v-7H7.5zM5 12h2v7h10v-7h2v9H5z\"/></svg>";

/// Share this track: its album, cued to it. Only where the page has a
/// `#share-result` to show the link in.
fn share_track_button(t: &TrackRow) -> String {
    match t.album_id {
        Some(album) => format!(
            "<button class=\"quiet\" data-act-share data-indicator:_sharing data-attr:disabled=\"$_sharing\" \
data-on:click=\"@post('/album/{album}/share?track={id}')\" aria-label=\"Share this track\" \
title=\"Share this track\">{ICON_SHARE}</button>",
            id = t.id
        ),
        None => String::new(),
    }
}

/// A track row. `album` is set where the row stands alone (search) and names
/// the record it comes from.
fn track_row(
    t: &TrackRow,
    n: usize,
    show_artist: bool,
    album: bool,
    versions: &Versions,
    share: bool,
    hearts: Option<&Hearts>,
) -> String {
    let mut sub = Vec::new();
    if show_artist {
        sub.push(escape(&t.artist_name));
    }
    if album {
        sub.push(escape(&t.album_title));
    }
    let sub = if sub.is_empty() {
        String::new()
    } else {
        format!("<small>{}</small>", sub.join(" · "))
    };
    format!(
        "<li tabindex=0 data-id={id} data-dur={secs} data-title=\"{title}\" data-artist=\"{artist}\" \
data-album=\"{album_title}\" data-album-id={album_id} data-artist-id={artist_id} data-cover=\"{cover}\">\
<span class=\"n\">{n}</span><span class=\"t\">{title}{sub}</span><span class=\"d\">{dur}</span>\
<span class=\"contents max-wide:hidden\">{heart}{share}\
<button class=\"quiet\" data-act=add aria-label=\"Add to queue\" title=\"Add to queue\">+</button></span>{mark}{MORE}</li>",
        id = t.id,
        secs = t.duration_ms.unwrap_or(0) / 1000,
        title = escape(&t.title),
        artist = escape(&t.artist_name),
        album_title = escape(&t.album_title),
        album_id = t.album_id.unwrap_or(0),
        artist_id = t.artist_id.unwrap_or(0),
        cover = t
            .album_id
            .map(|a| cover_url(a, crate::covers::LARGE, versions))
            .unwrap_or_default(),
        dur = duration(t.duration_ms),
        heart = hearts.map(|h| h.track(t.id)).unwrap_or_default(),
        mark = hearts.map(Hearts::mark).unwrap_or_default(),
        share = if share {
            share_track_button(t)
        } else {
            String::new()
        },
    )
}

/// Every album `b` lets through, in its order. Whole, as the apps list them:
/// the covers load lazily, so the page costs markup and not images.
fn album_list(
    s: &UiState,
    user: &AuthUser,
    b: &Browse,
) -> Option<(Vec<AlbumRow>, Versions, Option<Hearts>)> {
    let db = open(&s.pool)?;
    let albums = queries::list_albums(&db.conn, &b.albums(user.user_id, shelves::now())).ok()?;
    let versions = album_versions(&db.conn, &albums);
    Some((albums, versions, Hearts::load(&db.conn, user)))
}

/// "12 albums", once a filter is on: the count a shelf's heading gave.
fn counted(n: usize, one: &str, many: &str, b: &Browse) -> String {
    if b.filtered() {
        format!(
            "<p class=\"{SUB}\">{n} {}</p>",
            if n == 1 { one } else { many }
        )
    } else {
        String::new()
    }
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
    let Some(((albums, versions, hearts), options)) = found else {
        return unavailable();
    };
    let grid = if albums.is_empty() {
        format!("<p class=\"{EMPTY}\">No albums match.</p>")
    } else {
        format!(
            "<div class=\"{GRID}\" id=albums>{}</div>",
            cells(&albums, &versions, hearts.as_ref())
        )
    };
    let inner = format!(
        "<h1>Albums</h1>{}{}{grid}",
        counted(albums.len(), "album", "albums", &b),
        browse::toolbar(&b, Kind::Albums, &options.0, &options.1)
    );
    respond(&s, &headers, &user, "Albums", &inner)
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
        Some((album, tracks, versions, Hearts::load(&db.conn, &who)))
    })
    .await;
    let Some((album, tracks, versions, hearts)) = found else {
        return not_found();
    };
    let can_share = user.role.has_permission(Role::User);
    let discs = tracks
        .iter()
        .map(|t| t.disc.unwrap_or(1))
        .collect::<std::collections::BTreeSet<_>>();
    let mut rows = String::new();
    let mut disc = None;
    for (i, t) in tracks.iter().enumerate() {
        if discs.len() > 1 && disc != Some(t.disc.unwrap_or(1)) {
            disc = Some(t.disc.unwrap_or(1));
            let _ = write!(rows, "<li class=\"disc\">Disc {}</li>", t.disc.unwrap_or(1));
        }
        let n = t.track_number.map_or(i + 1, |n| n as usize);
        rows.push_str(&track_row(
            t,
            n,
            t.artist_name != album.artist_name,
            false,
            &versions,
            can_share,
            hearts.as_ref(),
        ));
    }
    let total: i64 = tracks.iter().filter_map(|t| t.duration_ms).sum();
    let mut sub = vec![format!(
        "<a href=\"/artist/{}\">{}</a>",
        album.artist_id,
        escape(&album.artist_name)
    )];
    let y = year(album.date.as_deref());
    if !y.is_empty() {
        sub.push(escape(y));
    }
    sub.push(format!(
        "{} track{}",
        tracks.len(),
        if tracks.len() == 1 { "" } else { "s" }
    ));
    sub.push(duration(Some(total)));
    if let Some(codec) = &album.codec {
        sub.push(escape(codec));
    }
    let share = if can_share {
        format!(
            "<button class=\"standard\" data-indicator:_sharing data-attr:disabled=\"$_sharing\" \
data-class:busy=\"$_sharing\" data-on:click=\"@post('/album/{}/share')\">Share</button>",
            album.id
        )
    } else {
        String::new()
    };
    let inner = format!(
        "<header class=\"{HERO}\"><img class=\"{HERO_COVER}\" src=\"{cover}\" width={large} height={large} alt=\"\">\
<div class=\"min-w-0 flex-1\"><p class=\"{KICKER}\">Album</p><h1 class=\"mb-1 normal-case\">{title}</h1><p class=\"{SUB}\">{sub}</p>\
<div class=\"{ACTIONS}\">\
<button class=\"primary\" data-act=play>Play</button><button class=\"standard\" data-act=shuffle>Shuffle</button>\
<button class=\"standard\" data-act=queue>Add to queue</button>{share}{heart}</div><div id=share-result></div></div></header>\
<ol class=\"tracks\" data-context=album>{rows}</ol>",
        heart = hearts
            .as_ref()
            .map(|h| h.album(album.id))
            .unwrap_or_default(),
        cover = cover_url(album.id, crate::covers::LARGE, &versions),
        large = crate::covers::LARGE,
        title = escape(&album.title),
        sub = sub.join(" · "),
    );
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
    let html = match result {
        Ok(url) => format!(
            "<div id=share-result class=\"{COPY_ROW}\"><input id=share-url readonly value=\"{url}\" \
aria-label=\"Share link\" class=\"{COPY_INPUT}\">\
<button data-on:click=\"navigator.clipboard.writeText(document.getElementById('share-url').value)\">Copy</button></div>",
            url = escape(&url)
        ),
        Err(e) => format!(
            "<div id=share-result class=\"{COPY_ERROR}\" role=alert>{}</div>",
            escape(&e)
        ),
    };
    events(vec![patch(&html, None)])
}

fn artist_list(artists: &[queries::ArtistRow]) -> String {
    artists.iter().fold(String::new(), |mut out, a| {
        let _ = write!(
            out,
            "<li><a class=\"{LIST_ROW}\" href=\"/artist/{}\"><span class=\"{LIST_NAME}\">{}</span>\
<span class=\"{LIST_COUNT}\">{} album{}</span></a></li>",
            a.id,
            escape(&a.name),
            a.album_count,
            if a.album_count == 1 { "" } else { "s" }
        );
        out
    })
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
    let list = if artists.is_empty() {
        format!("<p class=\"{EMPTY}\">No artists match.</p>")
    } else {
        format!("<ul id=artists>{}</ul>", artist_list(&artists))
    };
    let inner = format!(
        "<h1>Artists</h1>{}{}{list}",
        counted(artists.len(), "artist", "artists", &b),
        browse::toolbar(&b, Kind::Artists, &options.0, &options.1)
    );
    respond(&s, &headers, &user, "Artists", &inner)
}

/// Tracks on one page of the track browser.
const TRACKS_PAGE: u32 = 200;

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
        let hearts = Hearts::load(&db.conn, &who);
        drop(db);
        Some((rows, total, versions, hearts, filter_options(&st)?))
    })
    .await;
    let Some((rows, total, versions, hearts, options)) = found else {
        return unavailable();
    };
    let first = (page * TRACKS_PAGE) as usize;
    let list = if rows.is_empty() {
        format!("<p class=\"{EMPTY}\">No tracks match.</p>")
    } else {
        let items: String = rows
            .iter()
            .enumerate()
            .map(|(i, t)| {
                track_row(
                    t,
                    first + i + 1,
                    true,
                    true,
                    &versions,
                    false,
                    hearts.as_ref(),
                )
            })
            .collect();
        format!("<ol class=\"tracks\" data-context=album>{items}</ol>")
    };
    let mut nav = Vec::new();
    let at = |p: u32| {
        let q = b.query();
        let sep = if q.is_empty() { "" } else { "&" };
        format!("/tracks?{q}{sep}page={p}")
    };
    if page > 0 {
        nav.push(format!(
            "<a class=\"lowercase\" href=\"{}\">Previous</a>",
            at(page - 1)
        ));
    }
    if (((page + 1) * TRACKS_PAGE) as u64) < total {
        nav.push(format!(
            "<a class=\"lowercase\" href=\"{}\">Next</a>",
            at(page + 1)
        ));
    }
    let nav = if nav.is_empty() {
        String::new()
    } else {
        format!("<p class=\"mt-5 flex gap-4\">{}</p>", nav.join(""))
    };
    let inner = format!(
        "<h1>Tracks</h1><p class=\"{SUB}\">{total} track{}</p>{}{list}{nav}",
        if total == 1 { "" } else { "s" },
        browse::toolbar(&b, Kind::Tracks, &options.0, &options.1)
    );
    respond(&s, &headers, &user, "Tracks", &inner)
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
        Some((artist, albums, versions, Hearts::load(&db.conn, &who)))
    })
    .await;
    let Some((artist, albums, versions, hearts)) = found else {
        return not_found();
    };
    // Sharing and favouriting are the same accounts' to do.
    let share = match &hearts {
        Some(h) => format!(
            "<div class=\"mb-5 {ACTIONS}\"><button class=\"standard\" data-indicator:_sharing data-attr:disabled=\"$_sharing\" \
data-class:busy=\"$_sharing\" data-on:click=\"@post('/artist/{id}/share')\">Share</button>{heart}</div>\
<div id=share-result></div>",
            id = artist.id,
            heart = h.artist(artist.id),
        ),
        None => String::new(),
    };
    let inner = format!(
        "<p class=\"{KICKER}\">Artist</p><h1 class=\"normal-case\">{}</h1><p class=\"{SUB}\">{} album{} · {} tracks</p>{share}\
<div class=\"{GRID}\">{}</div>",
        escape(&artist.name),
        artist.album_count,
        if artist.album_count == 1 { "" } else { "s" },
        artist.track_count,
        cells(&albums, &versions, hearts.as_ref())
    );
    respond(&s, &headers, &user, &artist.name, &inner)
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
    let list = if lists.is_empty() {
        format!("<p class=\"{EMPTY}\">No playlists yet.</p>")
    } else {
        let rows = lists.iter().fold(String::new(), |mut out, p| {
            let _ = write!(
                out,
                "<li><a class=\"{LIST_ROW}\" href=\"/playlist/{}\"><span class=\"{LIST_NAME}\">{}</span>\
<span class=\"{LIST_COUNT}\">{} track{} · {}</span></a></li>",
                p.id,
                escape(&p.name),
                p.track_count,
                if p.track_count == 1 { "" } else { "s" },
                duration(Some(p.duration_ms)),
            );
            out
        });
        format!("<ul>{rows}</ul>")
    };
    let inner = format!("<h1>Playlists</h1>{list}");
    respond(&s, &headers, &user, "Playlists", &inner)
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
        Some((list, tracks, versions, Hearts::load(&db.conn, &who)))
    })
    .await;
    let Some((list, tracks, versions, hearts)) = found else {
        return not_found();
    };
    let rows: String = tracks
        .iter()
        .enumerate()
        .map(|(i, t)| track_row(t, i + 1, true, true, &versions, false, hearts.as_ref()))
        .collect();
    let cover = tracks
        .iter()
        .find_map(|t| t.album_id)
        .map(|a| {
            format!(
                "<img class=\"{HERO_COVER}\" src=\"{}\" width={large} height={large} alt=\"\">",
                cover_url(a, crate::covers::LARGE, &versions),
                large = crate::covers::LARGE,
            )
        })
        .unwrap_or_default();
    let mut sub = Vec::new();
    if let Some(owner) = list.owner.as_deref().filter(|o| !o.is_empty()) {
        sub.push(escape(owner));
    }
    sub.push(format!(
        "{} track{}",
        tracks.len(),
        if tracks.len() == 1 { "" } else { "s" }
    ));
    sub.push(duration(Some(list.duration_ms)));
    let comment = list
        .comment
        .as_deref()
        .filter(|c| !c.is_empty())
        .map(|c| format!("<p class=\"{SUB}\">{}</p>", escape(c)))
        .unwrap_or_default();
    let inner = format!(
        "<header class=\"{HERO}\">{cover}<div class=\"min-w-0 flex-1\"><p class=\"{KICKER}\">Playlist</p>\
<h1 class=\"mb-1 normal-case\">{title}</h1><p class=\"{SUB}\">{sub}</p>{comment}<div class=\"{ACTIONS}\">\
<button class=\"primary\" data-act=play>Play</button>\
<button class=\"standard\" data-act=shuffle>Shuffle</button><button class=\"standard\" data-act=queue>Add to queue</button></div></div></header>\
<ol class=\"tracks\" data-context=album>{rows}</ol>",
        title = escape(&list.name),
        sub = sub.join(" · "),
    );
    respond(&s, &headers, &user, &list.name, &inner)
}

fn results(s: &UiState, user: &AuthUser, q: &str) -> String {
    let q = q.trim();
    if q.is_empty() {
        return "<div id=results></div>".into();
    }
    let shelf = Shelf::Search(q);
    let found = open(&s.pool).and_then(|db| {
        let summary =
            shelves::summary(&db.conn, shelf, user.user_id, shelves::now(), false).ok()?;
        let versions = shelf_versions(&db.conn, &summary);
        Some((summary, versions, Hearts::load(&db.conn, user)))
    });
    let Some((summary, versions, hearts)) = found else {
        return format!(
            "<div id=results><p class=\"{ERROR}\">The library is unavailable.</p></div>"
        );
    };
    if summary.is_empty() {
        return format!(
            "<div id=results><p class=\"{EMPTY}\">Nothing matches “{}”.</p></div>",
            escape(q)
        );
    }
    format!(
        "<div id=results>{}</div>",
        shelf_sections(&summary, shelf, &versions, hearts.as_ref())
    )
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
    // Without script the form is an ordinary GET; with it, results follow typing.
    let inner = format!(
        "<h1>Search</h1><form class=\"search\" action=\"/search\" method=get \
data-on:submit__prevent=\"@get('/search/results')\">\
<input class=\"w-full max-w-panel px-3 py-2.5 text-input\" type=search name=q value=\"{q}\" placeholder=\"Albums, artists, tracks\" autocomplete=off \
autocapitalize=none spellcheck=false enterkeyhint=search autofocus aria-label=Search data-bind:q \
data-init=\"$q && @get('/search/results')\" \
data-on:input__debounce.250ms=\"@get('/search/results')\"></form>{found}",
        q = escape(&q),
    );
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

/// The library's own lists, which the sidebar shows as links and a phone, with
/// one tab for them all, as this page.
pub(super) async fn library(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    let rows = [
        ("/playlists", "Playlists"),
        ("/tracks", "Tracks"),
        ("/recent", "Recently played"),
        ("/favourites", "Favourites"),
        ("/history", "History"),
    ]
        .iter()
        .fold(String::new(), |mut out, (href, name)| {
            let _ = write!(
                out,
                "<li><a class=\"{LIST_ROW}\" href=\"{href}\"><span class=\"{LIST_NAME}\">{name}</span></a></li>"
            );
            out
        });
    let inner = format!("<h1>Library</h1><ul>{rows}</ul>");
    respond(&s, &headers, &user, "Library", &inner)
}

/// An artist as a pill, on a shelf.
fn pills(artists: &[queries::ArtistRow]) -> String {
    artists.iter().fold(String::new(), |mut out, a| {
        let _ = write!(
            out,
            "<a class=\"inline-flex max-w-full border border-muted px-3 py-1.5 text-control text-ink \
hover:bg-hover/30 hover:no-underline\" href=\"/artist/{}\"><span class=\"truncate\">{}</span></a>",
            a.id,
            escape(&a.name)
        );
        out
    })
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

/// A section's heading: its name, how many the shelf has in all, and a
/// chevron, the whole of it a link to the browser with the shelf as its
/// filter. The preview below may show fewer.
fn section_head(title: &str, total: u64, shelf: Shelf, kind: Kind, extra: &str) -> String {
    let href = escape(&browse::shelf_browser(shelf, kind));
    format!(
        "<div class=\"{LIST_HEAD}\"><h2><a class=\"inline-flex items-center gap-1.5 text-[inherit] \
hover:text-brand hover:no-underline\" href=\"{href}\">{title}\
<span class=\"text-muted tabular-nums\">{total}</span>{ICON_CHEVRON}</a></h2>\
<div class=\"{ACTIONS}\">{extra}</div></div>"
    )
}

/// The previews of a shelf, as the apps' shelf lays them out: artists as
/// pills, records as tiles, and the tracks as a list that plays on from the
/// row picked.
fn shelf_sections(
    s: &Summary,
    shelf: Shelf,
    versions: &Versions,
    hearts: Option<&Hearts>,
) -> String {
    let mut out = String::new();
    if !s.artists.preview.is_empty() {
        let _ = write!(
            out,
            "{}<div class=\"mb-6 flex flex-wrap gap-2\">{}</div>",
            section_head("Artists", s.artists.total, shelf, Kind::Artists, ""),
            pills(&s.artists.preview)
        );
    }
    if !s.albums.preview.is_empty() {
        let _ = write!(
            out,
            "{}<div class=\"mb-6 {GRID}\">{}</div>",
            section_head("Albums", s.albums.total, shelf, Kind::Albums, ""),
            cells(&s.albums.preview, versions, hearts)
        );
    }
    if !s.tracks.preview.is_empty() {
        let rows: String = s
            .tracks
            .preview
            .iter()
            .enumerate()
            .map(|(i, t)| track_row(t, i + 1, true, true, versions, false, hearts))
            .collect();
        let _ = write!(
            out,
            "{}<ol class=\"tracks\" data-context=album>{rows}</ol>",
            section_head(
                "Tracks",
                s.tracks.total,
                shelf,
                Kind::Tracks,
                "<button class=\"standard\" data-act=play>Play</button>\
<button class=\"standard\" data-act=shuffle>Shuffle</button><button class=\"standard\" data-act=queue>Add to queue</button>",
            ),
        );
    }
    out
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
        Some((summary, versions, Hearts::load(&db.conn, &who)))
    })
    .await;
    let Some((summary, versions, hearts)) = found else {
        return unavailable();
    };
    let inner = if summary.is_empty() {
        format!(
            "<h1>{title}</h1><p class=\"{EMPTY}\">{}</p><p class=\"{EMPTY}\">{}</p>",
            empty.title, empty.detail
        )
    } else {
        format!(
            "<h1>{title}</h1>{}",
            shelf_sections(&summary, shelf, &versions, hearts.as_ref())
        )
    };
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

/// The queue lives in the browser, so the page is a frame the script fills.
pub(super) async fn queue(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    let inner = format!(
        "<header class=\"{HERO}\"><img class=\"{HERO_COVER} max-wide:max-w-[360px] max-wide:self-center\" \
data-np=cover alt=\"\" hidden><div class=\"min-w-0 flex-1\"><p class=\"{KICKER}\">Now playing</p>\
<h1 class=\"mb-1 normal-case\" data-np=title>Nothing playing</h1>\
<p class=\"{SUB}\"><span data-np=artist></span> <a data-np=album href=\"/albums\"></a></p>\
{buttons}{scrub}</div></header>\
<div class=\"{LIST_HEAD}\"><h2>Up next</h2>\
<button class=\"quiet px-2 py-1\" data-act=clear>Clear</button></div>\
<ol id=queue-list class=\"tracks\"></ol>",
        buttons = buttons(true),
        scrub = scrub("max-w-form max-wide:mt-2 max-wide:text-meta", "h-11"),
    );
    respond(&s, &headers, &user, "Queue", &inner)
}
