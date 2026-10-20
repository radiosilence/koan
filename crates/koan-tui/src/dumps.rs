//! The browsing pages drawn against a library made to be awkward — long
//! names, CJK and emoji, missing years and numbers, a double album, a
//! compilation, favourites and plays — at the sizes terminals come in, as
//! text to read. Ignored by default: it fills this process's library, which
//! other tests expect empty. Run it on its own:
//!
//! ```text
//! cargo test -p koan-tui dumps -- --ignored
//! ```
//!
//! and read `target/tui-dumps/`.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use koan_core::db::queries::{self, TrackMeta};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::album_page::AlbumPage;
use super::browse::{Browse, Source, Thumbs, View};
use super::cover_art::Gfx;
use super::shelf_page::{ShelfKind, ShelfPage};
use super::theme::Theme;

const SIZES: [(u16, u16); 4] = [(50, 20), (80, 24), (120, 40), (200, 60)];

fn meta(artist: &str, album: &str, title: &str, n: Option<i32>) -> TrackMeta {
    TrackMeta {
        title: title.into(),
        artist: artist.into(),
        album_artist: Some(artist.into()),
        album: album.into(),
        date: Some("2007-05-01".into()),
        disc: Some(1),
        track_number: n,
        genre: Some("Electronic".into()),
        label: None,
        duration_ms: Some(215_000),
        codec: Some("FLAC".into()),
        sample_rate: Some(44_100),
        bit_depth: Some(16),
        channels: Some(2),
        bitrate: Some(900),
        size_bytes: Some(20_000_000),
        mtime: Some(1_700_000_000),
        path: Some(format!("/nowhere/{album}/{title}.flac")),
        server_path: None,
        source: "local".into(),
        remote_id: None,
        remote_url: None,
        album_remote_id: None,
        artist_remote_id: None,
        mbid: None,
        album_mbid: None,
        album_added_at: None,
        credits: Default::default(),
    }
}

/// The awkward library. Returns the ids of the double album and the
/// compilation.
pub(crate) fn seed() -> (i64, i64) {
    let db = koan_core::db::pool::shared().get().expect("a library");
    let c = &db.conn;
    let mut ids = Vec::new();
    for (artist, album, titles) in [
        ("Pnau", "PNAU", vec!["Embrace", "Wild Strawberries", "Baby"]),
        (
            "東京事変",
            "大発見",
            vec!["新しい文明開化", "電波通信", "天国へようこそ"],
        ),
        (
            "Godspeed You! Black Emperor",
            "Lift Your Skinny Fists Like Antennas to Heaven",
            vec![
                "Storm: Lift Yr. Skinny Fists, Like Antennas to Heaven…",
                "Static",
            ],
        ),
        (
            "🎵 Emoji Band 🔥",
            "Vibes 🎶",
            vec!["🔥 track", "plain track"],
        ),
        ("Polar Bear", "Same as You", vec!["Life, Love and Light"]),
    ] {
        for (i, title) in titles.iter().enumerate() {
            ids.push(
                queries::upsert_track(c, &meta(artist, album, title, Some(i as i32 + 1))).unwrap(),
            );
        }
    }
    // No year, no track numbers.
    let mut undated = meta("Nobody Knows", "Untitled Tape", "side a", None);
    undated.date = None;
    ids.push(queries::upsert_track(c, &undated).unwrap());
    // A double album.
    let mut double = 0;
    for (disc, n, title) in [
        (1, 1, "One"),
        (1, 2, "Two"),
        (2, 1, "Three"),
        (2, 2, "Four"),
    ] {
        let mut m = meta("Burial", "Untrue (Deluxe)", title, Some(n));
        m.disc = Some(disc);
        let id = queries::upsert_track(c, &m).unwrap();
        double = queries::get_track_row(c, id)
            .unwrap()
            .unwrap()
            .album_id
            .unwrap();
        ids.push(id);
    }
    // A compilation: one album artist, many track artists.
    let mut comp = 0;
    for (i, artist) in ["Four Tet", "Floating Points", "Caribou"]
        .iter()
        .enumerate()
    {
        let mut m = meta(
            artist,
            "Late Night Tales",
            &format!("cut {i}"),
            Some(i as i32 + 1),
        );
        m.album_artist = Some("Various Artists".into());
        let id = queries::upsert_track(c, &m).unwrap();
        comp = queries::get_track_row(c, id)
            .unwrap()
            .unwrap()
            .album_id
            .unwrap();
        ids.push(id);
    }
    let user = queries::LOCAL_USER;
    queries::set_favourite_album(c, user, double, true).unwrap();
    for id in ids.iter().step_by(3) {
        queries::add_favourite(c, user, *id).unwrap();
    }
    let now = koan_core::shelves::now();
    for (i, id) in ids.iter().enumerate().take(6) {
        queries::record_play_at(c, user, *id, now - i as i64 * 600, Some(1000), "local").unwrap();
    }
    if let Ok(artists) = queries::list_artists(c, &Default::default()) {
        for a in artists.iter().take(2) {
            queries::set_favourite_artist(c, user, a.id, true).unwrap();
        }
    }
    (double, comp)
}

/// The buffer as lines of text, a wide character's second cell left out.
fn text(buf: &Buffer) -> String {
    let mut out = String::new();
    for y in 0..buf.area.height {
        let mut line = String::new();
        let mut skip = 0;
        for x in 0..buf.area.width {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let sym = buf[(x, y)].symbol();
            skip = unicode_width::UnicodeWidthStr::width(sym).saturating_sub(1);
            line.push_str(sym);
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

fn out_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/tui-dumps");
    std::fs::create_dir_all(&dir).ok();
    dir
}

/// Waits for a page's read to land.
fn settle(mut poll: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(5);
    while !poll() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "fills the library; run on its own with --ignored"]
fn dumps() {
    koan_core::config::isolate_config_for_tests();
    let (double, comp) = seed();
    let theme = Theme::default();
    let favourites: HashSet<i64> = queries::load_favourites(
        &koan_core::db::pool::shared().get().unwrap().conn,
        queries::LOCAL_USER,
    )
    .unwrap_or_default();
    let dir = out_dir();
    let write = |name: &str, w: u16, h: u16, draw: &mut dyn FnMut(Rect, &mut Buffer)| {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        draw(area, &mut buf);
        std::fs::write(dir.join(format!("{name}-{w}x{h}.txt")), text(&buf)).unwrap();
    };

    for (name, source, view) in [
        ("albums-grid", Source::Albums, View::Tiles),
        ("albums-list", Source::Albums, View::List),
        ("artists", Source::Artists, View::List),
        ("tracks", Source::Tracks, View::List),
    ] {
        let mut page = Browse::new(source);
        page.view = view;
        page.reload();
        settle(|| page.poll());
        for (w, h) in SIZES {
            write(name, w, h, &mut |area, buf| {
                page.render(
                    area,
                    buf,
                    &theme,
                    &mut Gfx::halfblocks(),
                    &mut Thumbs::new(),
                    &favourites,
                    None,
                )
            });
        }
    }

    for (name, kind, query) in [
        ("favourites", ShelfKind::Favourites, ""),
        ("recent", ShelfKind::Recent, ""),
        ("downloaded", ShelfKind::Downloaded, ""),
        ("search", ShelfKind::Search, "e"),
    ] {
        let mut page = ShelfPage::new(kind);
        page.query = query.into();
        page.typing = false;
        page.reload();
        settle(|| page.poll());
        for view in [View::Tiles, View::List] {
            if view == View::List {
                page.toggle_view();
            }
            let name = format!(
                "{name}-{}",
                if view == View::Tiles { "grid" } else { "list" }
            );
            for (w, h) in SIZES {
                write(&name, w, h, &mut |area, buf| {
                    page.render(
                        area,
                        buf,
                        &theme,
                        &mut Gfx::halfblocks(),
                        &mut Thumbs::new(),
                        &favourites,
                        None,
                    )
                });
            }
        }
    }

    // An artist with albums of their own, and one who only appears on the
    // compilation.
    let (own, guest) = {
        let db = koan_core::db::pool::shared().get().unwrap();
        let tracks = queries::tracks_for_album(&db.conn, comp).unwrap();
        let double_artist = queries::get_album(&db.conn, double)
            .unwrap()
            .unwrap()
            .artist_id;
        (double_artist, tracks[0].artist_id.unwrap())
    };
    for (name, id) in [("artist", own), ("artist-appears-on", guest)] {
        let mut page = Browse::new(Source::Artist(id));
        page.reload();
        settle(|| {
            page.poll();
            !page.is_empty()
        });
        for (w, h) in SIZES {
            write(name, w, h, &mut |area, buf| {
                page.render(
                    area,
                    buf,
                    &theme,
                    &mut Gfx::halfblocks(),
                    &mut Thumbs::new(),
                    &favourites,
                    None,
                )
            });
        }
    }

    for (name, id) in [("album-double", double), ("album-compilation", comp)] {
        let mut page = AlbumPage::load(id).expect("the album");
        for (w, h) in SIZES {
            write(name, w, h, &mut |area, buf| {
                page.render(
                    area,
                    buf,
                    &theme,
                    &mut Gfx::halfblocks(),
                    &mut Thumbs::new(),
                    &favourites,
                    None,
                )
            });
        }
    }
}
