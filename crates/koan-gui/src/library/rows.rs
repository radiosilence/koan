//! One track in a list, shared by a record's page, an artist's page and the
//! track browser: its number, or the bars while it is the current track
//! (flat while paused), or a play mark under the pointer; the sleeve where
//! the list gathers many records; the title, over the artist and record
//! where they differ from the page's; where the file is; the heart; the
//! format; the length.
//!
//! Lists select as lists do: a click picks the row, ⌘ adds or removes one,
//! ⇧ extends, and a double click plays the list from it.

use koan_ffi::{Track, TransferState};
use koan_kit::app::*;
// text measured or shaped at runtime.
use koan_kit::raw::font;
// sizes and colours computed at runtime from measurements, not chosen from the theme.
use koan_kit::raw::px;

use super::actions;
use super::covers::Sleeve;
use super::drag::{Playable, PlayableDrag};
use super::format;
use super::selection::Selection;
use super::tokens::{Paint, Type};
use crate::components::{
    AvailabilityMark, Cover, CoverArt, ROW_GROUP, RowKind, RowMetrics, RowNumber, TableRow,
};

/// A row of one line: the title alone.
pub fn height(p: &Paint) -> Pixels {
    RowMetrics::height(RowKind::Line, &p.look)
}

/// A row with a credit line under its title.
pub fn text_height(p: &Paint) -> Pixels {
    RowMetrics::height(RowKind::Text, &p.look)
}

/// A row that carries its sleeve, with the artist and record under the title.
pub fn art_height(p: &Paint) -> Pixels {
    RowMetrics::height(RowKind::Art, &p.look)
}

/// A gathered row's sleeve, in points at 100 % (`RowMetrics::sleeve`).
pub const SLEEVE: f32 = 32.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowPart {
    Row,
    Artist(i64),
    Album(i64),
    Heart,
}

pub trait TrackList: actions::Menus {
    fn clicked(
        &mut self,
        index: usize,
        track: i64,
        part: RowPart,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    );

    /// The list's tracks in the order they play, for a row's play button to
    /// play from that row with the rest behind it, as a double click does.
    fn row_tracks(&self) -> Vec<i64>;
}

pub struct RowState {
    pub lead: SharedString,
    pub lead_width: Pixels,
    /// The track under the cursor: its bars, moving while it plays and
    /// flat while it is paused, in place of its number.
    pub current: bool,
    pub favourite: bool,
    pub selected: bool,
    /// Gathered from many records: the sleeve, and the record named.
    pub gathered: bool,
    pub transfer: Option<TransferState>,
    /// What dragging the row carries, and what its menu acts on: the list's
    /// selection when the row is in it, built once for the selection.
    pub drag: PlayableDrag,
    /// The list's row kind: one for every row, since a list lays out
    /// uniform rows (`height`, `text_height` or `art_height`).
    pub kind: RowKind,
    /// A glyph before the figures, with what it means under the pointer: a
    /// play another client scrobbled in, in the history.
    pub note: Option<(crate::theme::Icon, &'static str)>,
}

#[allow(clippy::too_many_arguments)]
pub fn track_row<V: TrackList>(
    p: &Paint,
    track: &Track,
    index: usize,
    state: RowState,
    sleeve: Option<Sleeve>,
    page_artist: Option<i64>,
    cx: &mut Context<V>,
) -> AnyElement {
    let p = &p.picked(state.selected);
    let id = track.id;
    let view = cx.weak_entity();
    let click = |part: RowPart| {
        super::listen(
            view.clone(),
            move |this: &mut V, e: &ClickEvent, window, cx| {
                if part != RowPart::Row {
                    cx.stop_propagation();
                }
                this.clicked(index, id, part, e, window, cx)
            },
        )
    };
    let play = {
        let view = view.clone();
        move |_: &mut Window, cx: &mut App| {
            view.update(cx, |this, cx| {
                let ids = this.row_tracks();
                actions::play_list(this.lib(), ids, index, cx);
            })
            .ok();
        }
    };
    let lead = RowNumber::new(("row-play", id as u64), state.lead.clone())
        .current(state.current)
        .when(state.current, |n| n.bars(crate::levels::bars(p.accent)))
        .on_play(play);

    let names_artist = state.gathered
        || track.credit_differs
        || page_artist.is_some_and(|a| track.artist_id != Some(a));
    let mut detail = div().flex();
    if names_artist {
        detail = detail.children(track.artist_credit.iter().enumerate().map(|(i, part)| {
            let span = div()
                .id(("row-credit", (id as u64) << 8 | i as u64))
                .flex_none()
                .child(part.text.clone());
            match part.artist_id {
                Some(artist) => span
                    .cursor_pointer()
                    .hover(|s| s.underline().text_color(p.ink))
                    .on_click(click(RowPart::Artist(artist)))
                    .into_any_element(),
                None => span.into_any_element(),
            }
        }));
    }
    if state.gathered
        && let Some(album) = track.album_id
    {
        detail = detail
            .child(div().flex_none().child("\u{a0}·\u{a0}"))
            .child(
                div()
                    .id(("row-album", id as u64))
                    .cursor_pointer()
                    .hover(|s| s.underline().text_color(p.ink))
                    .on_click(click(RowPart::Album(album)))
                    .child(track.album_title.clone()),
            );
    }

    let heart = crate::components::Heart::new(("row-heart", id as u64), state.favourite)
        .when(!state.favourite, |d| {
            d.invisible().group_hover(ROW_GROUP, |s| s.visible())
        })
        .on_click(click(RowPart::Heart));

    let drag = state.drag;
    let target = actions::MenuTarget {
        items: drag.items.clone(),
        album: track.album_id,
        after: None,
    };

    let sleeve = state.gathered.then(|| {
        Cover::new(
            sleeve.map_or(CoverArt::Missing, CoverArt::from),
            RowMetrics::sleeve(&p.look),
        )
    });

    TableRow::new(("row", id as u64), state.kind)
        .selected(state.selected)
        .lead_exact(state.lead_width, lead)
        .when_some(sleeve, |row, sleeve| row.lead(SLEEVE, sleeve))
        .title(
            div()
                .text_color(if state.current { p.accent } else { p.ink })
                .child(track.title.clone()),
        )
        .when(state.gathered || names_artist, |row| row.line(detail))
        .when_some(state.note, |row, (icon, meaning)| {
            row.figure(
                16.0,
                div()
                    .id(("row-note", id as u64))
                    .flex()
                    .justify_center()
                    .tooltip(move |window, cx| {
                        crate::components::Tooltip::view(meaning.into(), window, cx)
                    })
                    .child(crate::theme::KoanIcon::new(icon).color(p.muted)),
            )
        })
        .figure(
            14.0,
            AvailabilityMark::new(crate::transfers::of_track(
                track.id,
                track.on_server,
                track.on_disk,
                state.transfer,
            ))
            .picked(state.selected),
        )
        .figure(20.0, heart)
        .figure(110.0, div().children(format::quality(track)))
        .figure(64.0, format::duration(track.duration_ms))
        .on_click(click(RowPart::Row))
        .on_drag(drag, |drag: &PlayableDrag, at, _, cx| drag.preview(at, cx))
        .on_mouse_down(MouseButton::Right, actions::mark(view, target))
        .into_any_element()
}

/// A lead column wide enough for the longest number in a list of `count`,
/// measured in the face the numbers are drawn in.
pub fn lead_width(p: &Paint, count: usize, window: &Window) -> Pixels {
    let digits = count.max(1).to_string().len().max(2) as f32;
    let text = window.text_system();
    let font = text.resolve_font(&font(crate::theme::FONT_FAMILY));
    let size = p.look.font_size(Type::Meta);
    let digit = text
        .advance(font, size, '0')
        .map(|size| size.width)
        .unwrap_or(size * 0.6);
    digit * digits + px(4.0)
}

/// What a click on a row does to a list's selection: picks it alone, ⌘ adds
/// or removes it, ⇧ extends from the last one clicked, through the page's
/// order, which only ⇧ needs. Returns whether it was a double click on the
/// row, which plays.
pub fn select(
    selection: &mut Selection,
    item: Playable,
    event: &ClickEvent,
    page: impl FnOnce() -> Vec<Playable>,
    cx: &App,
) -> bool {
    let m = event.modifiers();
    if event.click_count() >= 2 && !m.shift && !m.secondary(cx) {
        return true;
    }
    if m.shift {
        selection.click(item, true, &page());
    } else if m.secondary(cx) {
        selection.click(item, false, &[]);
    } else {
        selection.only(item);
    }
    false
}
