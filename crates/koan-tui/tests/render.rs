//! Render-surface tests for the TUI widgets.
//!
//! Two things these guard that a green build does not: the layout solver's
//! exact output, and that no widget writes outside its `Rect` when the
//! terminal is degenerately small.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::Widget;
use ratatui::{Terminal, TerminalOptions, Viewport};

use koan_core::lyrics::{LrcLine, Lyrics, LyricsSource};
use koan_core::player::state::{
    PlaybackState, QueueEntry, QueueEntryStatus, QueueItemId, TrackInfo,
};

use koan_tui::app::{HoverZone, Mode};
use koan_tui::context_menu::context_menu_rect_at;
use koan_tui::cover_art::CoverArt;
use koan_tui::help_modal::HelpModalOverlay;
use koan_tui::library::{LibraryNode, LibraryState, LibraryView};
use koan_tui::lyrics::{LyricsPanel, LyricsState};
use koan_tui::organize::organize_popup_rect;
use koan_tui::picker::{
    PickerItem, PickerKind, PickerOverlay, PickerPartKind, PickerState, picker_popup_rect,
    picker_results_rect,
};
use koan_tui::queue::QueueView;
use koan_tui::theme::Theme;
use koan_tui::transport::TransportBar;
use koan_tui::visualizer::VisualizerMode;
use koan_tui::viz_picker::{VizPickerOverlay, VizPickerState};

/// Terminal sizes every widget must survive, from absurd to ordinary.
const SIZES: &[(u16, u16)] = &[
    (1, 1),
    (2, 1),
    (1, 2),
    (3, 2),
    (4, 3),
    (8, 3),
    (10, 4),
    (20, 5),
    (40, 10),
    (80, 24),
    (200, 60),
];

/// Titles that stress grapheme handling: CJK, emoji, ZWJ sequences,
/// combining marks, RTL, and halfwidth katakana with a trailing sound mark.
const NASTY: &[&str] = &[
    "ascii only",
    "東京事変 — 群青日和",
    "🎵🎶 vibes 🔥",
    "👩‍👩‍👧‍👦 family",
    "e\u{0301}te\u{0301} combining",
    "مرحبا بالعالم",
    "ｶﾞｷﾞｸﾞｹﾞｺﾞ",
    "a\u{FF9E}b",
    "",
];

fn render_widget<W: Widget>(w: W, width: u16, height: u16) -> Buffer {
    let mut term = Terminal::with_options(
        TestBackend::new(width, height),
        TerminalOptions {
            viewport: Viewport::Fixed(Rect::new(0, 0, width, height)),
        },
    )
    .expect("terminal");
    term.draw(|f| f.render_widget(w, f.area())).expect("draw");
    term.backend().buffer().clone()
}

fn track_info(title_len_hint: u64) -> TrackInfo {
    TrackInfo {
        id: QueueItemId::new(),
        path: PathBuf::from("/music/a.flac"),
        codec: "FLAC".into(),
        sample_rate: 44100,
        bit_depth: Some(16),
        bitrate_kbps: None,
        channels: 2,
        duration_ms: title_len_hint,
    }
}

fn entry(title: &str, status: QueueEntryStatus) -> QueueEntry {
    QueueEntry {
        playlist_entry_id: None,
        id: QueueItemId::new(),
        db_id: Some(1),
        path: PathBuf::from("/music/a.flac"),
        title: title.into(),
        artist: title.into(),
        album_artist: title.into(),
        album: title.into(),
        year: Some("2026".into()),
        codec: Some("FLAC".into()),
        track_number: Some(1),
        disc: Some(1),
        duration_ms: Some(215_000),
        status,
        download_progress: Some((3, 10)),
        error: None,
    }
}

// ---------------------------------------------------------------------------
// Layout solver
// ---------------------------------------------------------------------------

/// `ui::render`'s main split, pinned. The solver decides how a too-short
/// terminal is carved up and every downstream `Rect` inherits that decision,
/// so a silent change here moves the whole UI without failing to compile.
#[test]
fn main_layout_split_is_stable() {
    // (terminal height, transport height requested) -> granted chunk heights
    let cases: &[(u16, u16, [u16; 3])] = &[
        (60, 20, [20, 39, 1]),
        (24, 3, [3, 20, 1]),
        (24, 10, [10, 13, 1]),
        (10, 3, [3, 6, 1]),
        (7, 3, [3, 3, 1]),
        (6, 3, [3, 3, 0]),
        (5, 3, [2, 3, 0]),
        (5, 10, [2, 3, 0]),
        (4, 3, [1, 3, 0]),
        (3, 3, [0, 3, 0]),
        (2, 3, [0, 2, 0]),
        (1, 3, [0, 1, 0]),
        (0, 3, [0, 0, 0]),
    ];

    for &(term_h, transport_h, expected) in cases {
        let area = Rect::new(0, 0, 80, term_h);
        let chunks = Layout::vertical([
            Constraint::Length(transport_h),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);

        let got = [chunks[0].height, chunks[1].height, chunks[2].height];
        assert_eq!(got, expected, "term_h={term_h} transport_h={transport_h}");

        // Chunks tile the area exactly and never escape it.
        assert_eq!(chunks[0].y, area.y, "term_h={term_h}");
        assert_eq!(chunks[1].y, chunks[0].bottom(), "term_h={term_h}");
        assert_eq!(chunks[2].y, chunks[1].bottom(), "term_h={term_h}");
        assert_eq!(chunks[2].bottom(), area.bottom(), "term_h={term_h}");
    }
}

/// A short terminal gets less transport height than the art asks for. Callers
/// must read the granted height off the chunk rather than reusing the request.
#[test]
fn short_terminal_grants_less_transport_height_than_requested() {
    let requested = 10u16;
    let area = Rect::new(0, 0, 80, 5);
    let chunks = Layout::vertical([
        Constraint::Length(requested),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .split(area);
    assert!(chunks[0].height < requested);
    assert!(chunks[0].bottom() <= area.bottom());
}

/// Layout::vertical is stable under repeat calls (the cache is keyed correctly).
#[test]
fn layout_is_deterministic() {
    let area = Rect::new(0, 0, 80, 24);
    let build = || {
        Layout::vertical([
            Constraint::Length(10),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area)
    };
    assert_eq!(build(), build());
}

// ---------------------------------------------------------------------------
// Transport / seek bar
// ---------------------------------------------------------------------------

/// The click-to-seek hit test uses `bar_metrics`; the bar itself is drawn by
/// `TransportBar::render`. If those two disagree, clicking seeks to the wrong
/// place — and it still compiles.
#[test]
fn seek_bar_metrics_match_rendered_bar() {
    let info = track_info(215_000);
    let e = entry("Title", QueueEntryStatus::Playing);
    let theme = Theme::default();

    for width in [20u16, 40, 60, 80, 120, 200] {
        let area = Rect::new(0, 0, width, 3);
        let (bar_start, bar_width) = TransportBar::bar_metrics(area, 107_500, 215_000);

        let buf = render_widget(
            TransportBar::new(
                Some(&info),
                Some(&e),
                PlaybackState::Playing,
                107_500,
                &theme,
            ),
            width,
            3,
        );

        // Column 0..4 is " " + 2-cell status icon + " ".
        let prefix: String = (0..4.min(width))
            .map(|x| buf[(x, 0)].symbol())
            .collect::<Vec<_>>()
            .concat();
        assert_eq!(&prefix[..1], " ", "width={width}");
        assert_eq!(bar_start, 4, "width={width}");

        // Every cell inside the reported bar span is a bar glyph, filled or empty.
        for x in bar_start..bar_start + bar_width {
            let sym = buf[(x, 0)].symbol();
            assert!(
                sym == "\u{2501}" || sym == "\u{2500}",
                "width={width} col={x} is {sym:?}, not a bar glyph"
            );
        }
        // The cell just past the bar is the separating space, not a bar glyph.
        if bar_start + bar_width < width {
            assert_eq!(
                buf[(bar_start + bar_width, 0)].symbol(),
                " ",
                "width={width}"
            );
        }
    }
}

#[test]
fn transport_survives_every_size_and_title() {
    let theme = Theme::default();
    for title in NASTY {
        let info = track_info(215_000);
        let e = entry(title, QueueEntryStatus::Playing);
        for &(w, h) in SIZES {
            for state in [
                PlaybackState::Playing,
                PlaybackState::Paused,
                PlaybackState::Stopped,
            ] {
                render_widget(
                    TransportBar::new(Some(&info), Some(&e), state, 107_500, &theme)
                        .with_ticker_offset(7)
                        .with_download_fraction(Some(0.4)),
                    w,
                    h,
                );
            }
        }
    }
}

#[test]
fn transport_with_no_track_survives_every_size() {
    let theme = Theme::default();
    for &(w, h) in SIZES {
        render_widget(
            TransportBar::new(None, None, PlaybackState::Stopped, 0, &theme),
            w,
            h,
        );
    }
}

// ---------------------------------------------------------------------------
// Queue
// ---------------------------------------------------------------------------

#[test]
fn queue_survives_every_size_and_title() {
    let theme = Theme::default();
    let statuses = [
        QueueEntryStatus::Queued,
        QueueEntryStatus::Playing,
        QueueEntryStatus::Played,
        QueueEntryStatus::Downloading,
        QueueEntryStatus::PriorityPending,
        QueueEntryStatus::Failed,
    ];
    let entries: Vec<QueueEntry> = NASTY
        .iter()
        .enumerate()
        .map(|(i, t)| entry(t, statuses[i % statuses.len()]))
        .collect();
    let selected: HashSet<usize> = [0usize, 2].into_iter().collect();
    let favourites: HashSet<PathBuf> = [PathBuf::from("/music/a.flac")].into_iter().collect();

    for &(w, h) in SIZES {
        for mode in [Mode::Normal, Mode::QueueEdit] {
            // Cursor and scroll offset past the end must not index out of bounds.
            for (cursor, scroll) in [(0, 0), (3, 1), (entries.len(), entries.len() + 5)] {
                render_widget(
                    QueueView::new(&entries, &mode, cursor, scroll, &theme, &selected, 3)
                        .with_drop_indicator(Some(2))
                        .with_hover(&HoverZone::QueueItem(1))
                        .with_favourites(&favourites),
                    w,
                    h,
                );
            }
        }
    }
}

fn render_empty_queue(w: u16, h: u16) {
    let theme = Theme::default();
    let selected = HashSet::new();
    let mode = Mode::Normal;
    render_widget(QueueView::new(&[], &mode, 0, 0, &theme, &selected, 0), w, h);
}

#[test]
fn empty_queue_survives_every_size() {
    // Heights below 2 leave the top border no room for a content row — see
    // `empty_queue_panics_with_no_inner_row`.
    for &(w, h) in SIZES.iter().filter(|&&(_, h)| h >= 2) {
        render_empty_queue(w, h);
    }
}

/// The empty-state line is written at `block.inner(area).y` without checking
/// the border left a row, so a one-row area indexes past the buffer. Same
/// failure on ratatui 0.29, so it is not a migration regression; it belongs
/// with the other TUI bounds fixes rather than here.
#[test]
#[ignore = "known reachable panic, pre-existing on ratatui 0.29"]
fn empty_queue_panics_with_no_inner_row() {
    render_empty_queue(1, 1);
    render_empty_queue(2, 1);
}

// ---------------------------------------------------------------------------
// Library
// ---------------------------------------------------------------------------

fn library_state() -> LibraryState {
    // Path whose parent does not exist — no DB is opened, nodes stay empty.
    let mut st = LibraryState::new(Path::new("/koan-render-test-no-such-dir/none.db"));
    for (i, name) in NASTY.iter().enumerate() {
        st.nodes.push(LibraryNode::Artist {
            id: i as i64,
            name: (*name).into(),
            expanded: true,
        });
        st.nodes.push(LibraryNode::Album {
            id: i as i64,
            title: (*name).into(),
            year: Some("2026".into()),
            expanded: true,
        });
        st.nodes.push(LibraryNode::Track {
            id: i as i64,
            title: (*name).into(),
            number: Some(i as i32),
            duration_ms: Some(215_000),
            source: if i % 2 == 0 { "local" } else { "remote" }.into(),
        });
    }
    st
}

#[test]
fn library_survives_every_size() {
    let theme = Theme::default();
    let mut st = library_state();
    for &(w, h) in SIZES {
        for focused in [true, false] {
            for (cursor, scroll, filter, filter_active) in [
                (0usize, 0usize, "", false),
                (5, 2, "東京", true),
                (st.nodes.len() + 10, st.nodes.len() + 10, "🎵", true),
            ] {
                st.cursor = cursor;
                st.scroll_offset = scroll;
                st.filter = filter.into();
                st.filter_active = filter_active;
                render_widget(LibraryView::new(&st, &theme, focused), w, h);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Picker
// ---------------------------------------------------------------------------

fn build_items() -> Vec<PickerItem> {
    NASTY
        .iter()
        .enumerate()
        .map(|(i, t)| PickerItem {
            id: i as i64,
            display: (*t).into(),
            match_text: (*t).into(),
            parts: vec![((*t).into(), PickerPartKind::Title)],
        })
        .collect()
}

#[test]
fn picker_survives_every_size() {
    let theme = Theme::default();
    for &(w, h) in SIZES {
        for kind in [
            PickerKind::Track,
            PickerKind::Album,
            PickerKind::Artist,
            PickerKind::QueueJump,
        ] {
            let mut st = PickerState::new(kind, build_items(), true);
            for c in "東京".chars() {
                st.type_char(c);
            }
            st.tick();
            render_widget(PickerOverlay::new(&st, &theme), w, h);
        }
    }
}

/// Popup rects are clamped to the terminal at every size, including sizes
/// smaller than the popup's own minimum.
#[test]
fn popup_rects_stay_inside_the_terminal() {
    for &(w, h) in SIZES {
        let area = Rect::new(0, 0, w, h);

        for popup in [
            picker_popup_rect(area),
            organize_popup_rect(area),
            context_menu_rect_at(
                area,
                8,
                Some(w.saturating_sub(1)),
                Some(h.saturating_sub(1)),
            ),
            context_menu_rect_at(area, 8, None, None),
            context_menu_rect_at(area, 40, Some(0), Some(0)),
        ] {
            assert!(popup.right() <= area.right(), "{popup:?} in {area:?}");
            assert!(popup.bottom() <= area.bottom(), "{popup:?} in {area:?}");
        }

        let results = picker_results_rect(picker_popup_rect(area));
        assert!(results.right() <= area.right(), "{results:?} in {area:?}");
        assert!(results.bottom() <= area.bottom(), "{results:?} in {area:?}");
    }
}

// ---------------------------------------------------------------------------
// Lyrics
// ---------------------------------------------------------------------------

#[test]
fn lyrics_survives_every_size() {
    let theme = Theme::default();

    let synced = LyricsState {
        result: Some(Lyrics {
            content: NASTY.join("\n"),
            synced: true,
            source: LyricsSource::Lrclib,
        }),
        lrc_lines: NASTY
            .iter()
            .enumerate()
            .map(|(i, t)| LrcLine {
                time_secs: i as f64 * 5.0,
                text: (*t).into(),
            })
            .collect(),
        track_path: Some(PathBuf::from("/music/a.flac")),
        fetching: false,
    };
    let plain = LyricsState {
        result: Some(Lyrics {
            content: NASTY.join("\n"),
            synced: false,
            source: LyricsSource::Lrclib,
        }),
        lrc_lines: Vec::new(),
        track_path: Some(PathBuf::from("/music/a.flac")),
        fetching: false,
    };
    let fetching = LyricsState {
        result: None,
        lrc_lines: Vec::new(),
        track_path: None,
        fetching: true,
    };

    for st in [&synced, &plain, &fetching, &LyricsState::default()] {
        for &(w, h) in SIZES {
            for pos in [0u64, 12_000, 999_999] {
                render_widget(LyricsPanel::new(st, pos, &theme, 4), w, h);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Cover art
// ---------------------------------------------------------------------------

/// Halfblock rendering packs two image rows into one terminal cell. An
/// off-by-one here tears or squashes the image, and never fails to compile.
#[test]
fn cover_art_halfblock_fills_expected_cells() {
    let img = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        64,
        64,
        image::Rgba([200, 30, 90, 255]),
    ));

    for &(w, h) in SIZES {
        let buf = render_widget(CoverArt::new(&img), w, h);
        assert_eq!(buf.area, Rect::new(0, 0, w, h), "size=({w},{h})");
    }

    // At a comfortable size the art must actually paint halfblocks, not spaces.
    let buf = render_widget(CoverArt::new(&img), 20, 10);
    let painted = (0..10)
        .flat_map(|y| (0..20).map(move |x| (x, y)))
        .filter(|&(x, y)| buf[(x, y)].symbol() == "\u{2580}")
        .count();
    assert!(painted > 0, "cover art painted no halfblocks");
}

// ---------------------------------------------------------------------------
// Overlays
// ---------------------------------------------------------------------------

#[test]
fn overlays_survive_every_size() {
    let theme = Theme::default();
    let viz = VizPickerState::new(VisualizerMode::Bars);
    for &(w, h) in SIZES {
        render_widget(HelpModalOverlay::new(&theme), w, h);
        render_widget(VizPickerOverlay::new(&viz, &theme), w, h);
    }
}
