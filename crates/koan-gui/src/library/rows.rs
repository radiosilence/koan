//! One track in a list, shared by a record's page, an artist's page and the
//! track browser: its number, or the bars while it is the current track
//! (flat while paused), or a play mark under the pointer; the sleeve where
//! the list gathers many records; the title, over the artist and record
//! where they differ from the page's; where the file is; the heart; the
//! format; the length.
//!
//! Lists select as lists do: a click picks the row, ⌘ adds or removes one,
//! ⇧ extends, and a double click plays the list from it.

use koan_kit::app::*;
use koan_runtime::{Track, TransferState};

use super::Lib;
use super::actions;
use super::covers::Sleeve;
use super::drag::{Playable, PlayableDrag};
use super::format;
use super::nav::Page;
use super::selection::Selection;
use super::tokens::Paint;
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

/// Where a click on a track's row landed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowPart {
    /// The row itself, which each list answers in its own way.
    Row,
    Artist(i64),
    Album(i64),
    Heart,
}

impl RowPart {
    /// What a click on a row's link or heart does, the same in every list:
    /// the artist's or the record's page, or the track's favourite.
    pub fn follow(self, track: i64, lib: &Lib, cx: &mut App) {
        match self {
            RowPart::Row => {}
            RowPart::Artist(id) => lib.nav.update(cx, |n, cx| n.open(Page::Artist(id), cx)),
            RowPart::Album(id) => lib.nav.update(cx, |n, cx| n.open(Page::Album(id), cx)),
            RowPart::Heart => lib
                .feed
                .update(cx, |f, cx| f.toggle_favourite(Playable::Track(track), cx)),
        }
    }
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
    /// Under the list's keyboard cursor: where its ring tells the list it
    /// was laid out.
    pub cursor: Option<crate::components::CursorSpot>,
    /// Its place in the list, of how many, for a screen reader.
    pub place: Option<(usize, usize)>,
    /// Where it sits in a list that draws only what is on screen, to scroll
    /// its neighbours in when a screen reader moves to it.
    pub reveal: Option<Reveal>,
}

/// A row's list and its index in it.
pub enum Reveal {
    Rows(crate::components::RowListState, usize),
    Uniform(crate::components::UniformListScrollHandle, usize, usize),
    /// An item of a `list` of mixed items, as the artist page's.
    List(crate::components::ListState, usize),
}

impl Reveal {
    pub fn run(&self) {
        match self {
            Reveal::Rows(list, ix) => list.reveal_around(*ix),
            Reveal::Uniform(handle, ix, count) => {
                crate::components::reveal_around(handle, *ix, *count)
            }
            Reveal::List(list, ix) => crate::components::reveal_around_list(list, *ix),
        }
    }
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
    // The row's parts are known by the track and its place in the list: a
    // list such as the history draws one track several times, and rows
    // sharing ids would share their pointer's state.
    let key = (id as u64 & 0xffff_ffff) | ((index as u64 & 0xff_ffff) << 32);
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
    let lead = RowNumber::new(("row-play", key), state.lead.clone())
        .current(state.current)
        .picked(state.selected)
        .when(state.current, |n| n.bars(crate::levels::bars(p.accent)))
        .on_play(play);

    let names_artist = state.gathered
        || track.credit_differs
        || page_artist.is_some_and(|a| track.artist_id != Some(a));
    let mut detail = div().flex();
    if names_artist {
        detail = detail.children(track.artist_credit.iter().enumerate().map(|(i, part)| {
            let span = div()
                .id(("row-credit", key << 8 | i as u64))
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
                    .id(("row-album", key))
                    .cursor_pointer()
                    .hover(|s| s.underline().text_color(p.ink))
                    .on_click(click(RowPart::Album(album)))
                    .child(track.album_title.clone()),
            );
    }

    let heart = crate::components::Heart::new(("row-heart", key), state.favourite)
        // A pointer finds an empty heart by hovering the row; a phone has no
        // hover, so there the outline always shows, as in the iOS app.
        .when(!state.favourite && !p.look.touch, |d| {
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

    let spoken = p.a11y.then(|| {
        let length = track
            .duration_ms
            .filter(|ms| *ms > 0)
            .map(|ms| crate::time::clock(ms as u64))
            .unwrap_or_default();
        crate::components::spoken([
            track.title.as_str(),
            if names_artist {
                track.artist_name.as_str()
            } else {
                ""
            },
            if state.gathered {
                track.album_title.as_str()
            } else {
                ""
            },
            length.as_str(),
            if state.current { "playing" } else { "" },
            if state.favourite { "favourite" } else { "" },
        ])
    });
    TableRow::new(("row", key), state.kind)
        .when_some(spoken, |row, spoken| row.spoken(spoken))
        .when_some(state.place.filter(|_| p.a11y), |row, (ix, count)| {
            row.place(ix, count)
        })
        .when_some(state.reveal, |row, reveal| {
            row.on_reveal(move |_, _| reveal.run())
        })
        .selected(state.selected)
        .cursor(state.cursor.is_some())
        .when_some(state.cursor.as_ref(), |row, spot| row.spot(spot))
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
                    .id(("row-note", key))
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
            .id(("row-availability", key))
            .picked(state.selected),
        )
        .figure(if p.look.touch { 44.0 } else { 20.0 }, heart)
        // A phone's row leaves the format to its record, as on iOS, and its
        // length takes no more than its digits.
        .when(!p.look.touch, |row| {
            row.figure(110.0, div().children(format::quality(track)))
        })
        .figure(
            if p.look.touch { 44.0 } else { 64.0 },
            format::duration(track.duration_ms),
        )
        .on_click(click(RowPart::Row))
        .on_drag(drag, |drag: &PlayableDrag, at, _, cx| drag.preview(at, cx))
        .on_mouse_down(MouseButton::Right, actions::mark(view, target))
        .into_any_element()
}

/// A lead column wide enough for the longest number in a list of `count`,
/// measured in the face the numbers are drawn in.
pub fn lead_width(p: &Paint, count: usize, window: &Window) -> Pixels {
    crate::components::RowMetrics::lead_width(count, &p.look, window)
}

/// What a click on a row does to a list's selection: picks it alone, ⌘ adds
/// or removes it, ⇧ extends from the last one clicked, through the page's
/// order, which only ⇧ needs. Returns whether it was a double click on the
/// row, which plays, or under touch a tap outside picking.
pub fn select(
    selection: &mut Selection,
    item: Playable,
    event: &ClickEvent,
    page: impl FnOnce() -> Vec<Playable>,
    cx: &App,
) -> bool {
    let m = event.modifiers();
    // A touch screen has no click to pick with: a tap plays or opens the
    // row, and picks it only while picking (the Select button's mode).
    if cx.global::<crate::theme::KoanTheme>().touch {
        // A quick second tap is the same tap again, not a second play.
        if event.click_count() > 1 {
            return false;
        }
        if !selection.is_active() {
            return true;
        }
        selection.click(item, false, &[]);
        return false;
    }
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
