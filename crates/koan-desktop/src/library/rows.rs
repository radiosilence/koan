//! One track in a list, shared by a record's page, an artist's page and the
//! track browser: its number, or the bars when it is playing, or a play mark
//! under the pointer; the sleeve where the list gathers many records; the
//! title, over the artist and record where they differ from the page's; where
//! the file is; the heart; the format; the length.
//!
//! Lists select as lists do: a click picks the row, ⌘ adds or removes one,
//! ⇧ extends, and a double click plays the list from it.

use gpui_kit::component::menu::ContextMenuExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use koan_ffi::{Track, TransferState};

use super::covers::Sleeve;
use super::drag::{Playable, PlayableDrag};
use super::format;
use super::selection::Selection;
use super::tile::playing_bars;
use super::tokens::{self, Type, Typeset, space};
use super::{Lib, actions};

/// The rows' padding above and below, on the spacing scale.
fn padding() -> Pixels {
    space::s4()
}

/// A row of one line: the title alone.
pub fn height() -> Pixels {
    px(24.0) + padding() * 2.0
}

/// A row with a credit line under its title.
pub fn text_height() -> Pixels {
    px(34.0) + padding() * 2.0
}

/// A row that carries its sleeve, with the artist and record under the title.
pub fn art_height() -> Pixels {
    px(40.0) + padding() * 2.0
}

pub const SLEEVE: f32 = 32.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowPart {
    Row,
    Artist(i64),
    Album(i64),
    Heart,
}

pub trait TrackList: Sized + 'static {
    fn lib(&self) -> &Lib;
    fn clicked(
        &mut self,
        index: usize,
        track: i64,
        part: RowPart,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    );
}

pub struct RowState {
    pub lead: SharedString,
    pub lead_width: Pixels,
    /// The track under the cursor, and whether it is playing.
    pub current: bool,
    pub playing: bool,
    pub favourite: bool,
    pub selected: bool,
    /// Gathered from many records: the sleeve, and the record named.
    pub gathered: bool,
    pub transfer: Option<TransferState>,
    /// What dragging the row carries, and what its menu acts on: the list's
    /// selection when the row is in it, built once for the selection.
    pub drag: PlayableDrag,
    /// The list's row height: one for every row, since a list lays out
    /// uniform rows (`height`, `text_height` or `art_height`).
    pub height: Pixels,
}

/// Where the file is, or how its download is going.
fn availability(track: &Track, transfer: Option<TransferState>) -> (Option<&'static str>, Rgba) {
    match transfer {
        Some(TransferState::Queued) => (Some("↓"), tokens::MUTED),
        Some(TransferState::Running) => (Some("↓"), tokens::ACCENT),
        Some(TransferState::Failed) => (Some("!"), tokens::BAD),
        _ => match (track.on_server, track.on_disk) {
            (true, true) => (Some("◉"), tokens::INK),
            (true, false) => (Some("○"), tokens::MUTED),
            (false, true) => (Some("●"), tokens::INK),
            (false, false) => (None, tokens::MUTED),
        },
    }
}

#[allow(clippy::too_many_arguments)]
pub fn track_row<V: TrackList>(
    track: &Track,
    index: usize,
    state: RowState,
    sleeve: Option<Sleeve>,
    page_artist: Option<i64>,
    host: &V,
    cx: &mut Context<V>,
) -> AnyElement {
    let id = track.id;
    let lib = host.lib().clone();
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
    let title_tone = if state.current {
        tokens::ACCENT
    } else {
        tokens::INK
    };

    let lead = div()
        .w(state.lead_width)
        .flex_none()
        .flex()
        .justify_end()
        .typeset(
            Type::Meta,
            if state.current {
                tokens::ACCENT
            } else {
                tokens::MUTED
            },
        )
        .child(if state.current && state.playing {
            playing_bars().into_any_element()
        } else {
            div()
                .child(
                    div()
                        .group_hover("row", |s| s.invisible())
                        .child(state.lead.clone()),
                )
                .into_any_element()
        });

    let credited = track.artist_credit.iter().enumerate().map(|(i, part)| {
        let span = div()
            .id(("row-credit", (id as u64) << 8 | i as u64))
            .child(part.text.clone());
        match part.artist_id {
            Some(artist) => span
                .cursor_pointer()
                .hover(|s| s.underline().text_color(tokens::INK))
                .on_click(click(RowPart::Artist(artist)))
                .into_any_element(),
            None => span.into_any_element(),
        }
    });
    let names_artist = state.gathered
        || track.credit_differs
        || page_artist.is_some_and(|a| track.artist_id != Some(a));
    let mut detail = div()
        .flex()
        .overflow_hidden()
        .whitespace_nowrap()
        .typeset(Type::Meta, tokens::MUTED);
    if names_artist {
        detail = detail.children(credited);
    }
    if state.gathered
        && let Some(album) = track.album_id
    {
        detail = detail.child(" · ").child(
            div()
                .id(("row-album", id as u64))
                .cursor_pointer()
                .hover(|s| s.underline().text_color(tokens::INK))
                .on_click(click(RowPart::Album(album)))
                .child(track.album_title.clone()),
        );
    }

    let title = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .child(
            div()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .typeset(Type::Body, title_tone)
                .child(track.title.clone()),
        )
        .when(state.gathered || names_artist, |d| d.child(detail));

    let (mark, mark_tone) = availability(track, state.transfer);
    let heart = div()
        .id(("row-heart", id as u64))
        .w(px(20.0))
        .flex_none()
        .cursor_pointer()
        .typeset(
            Type::Meta,
            if state.favourite {
                tokens::ACCENT
            } else {
                tokens::MUTED
            },
        )
        .when(!state.favourite, |d| {
            d.invisible().group_hover("row", |s| s.visible())
        })
        .on_click(click(RowPart::Heart))
        .child(if state.favourite { "♥" } else { "♡" });

    let drag = state.drag;
    let items = drag.items.clone();
    let album = track.album_id;

    let mut row = div()
        .id(("row", id as u64))
        .group("row")
        .flex()
        .items_center()
        .gap(space::s12())
        .px(px(super::tile::MARGIN))
        .h(state.height)
        .when(state.selected, |d| d.bg(tokens::SURFACE))
        .when(!state.selected, |d| {
            d.hover(|s| s.bg(tokens::alpha(tokens::HOVER, 0.3)))
        })
        .on_click(click(RowPart::Row))
        .child(lead);
    if state.gathered {
        let mut art = div().size(px(SLEEVE)).flex_none().bg(tokens::SURFACE);
        if let Some(Sleeve::Ready(image)) = sleeve {
            art = art.child(img(ImageSource::Render(image)).size_full());
        }
        row = row.child(art);
    }
    row.child(title)
        .child(
            div()
                .w(px(14.0))
                .flex_none()
                .typeset(Type::Fine, mark_tone)
                .children(mark),
        )
        .child(heart)
        .child(
            div()
                .w(px(110.0))
                .flex_none()
                .flex()
                .justify_end()
                .whitespace_nowrap()
                .typeset(Type::Meta, tokens::MUTED)
                .children(format::quality(track)),
        )
        .child(
            div()
                .w(px(64.0))
                .flex_none()
                .flex()
                .justify_end()
                .typeset(Type::Meta, tokens::MUTED)
                .child(format::duration(track.duration_ms)),
        )
        .on_drag(drag, |drag: &PlayableDrag, at, _, cx| drag.preview(at, cx))
        .context_menu(move |menu, window, cx| {
            actions::menu(menu, &lib, items.clone(), album, None, window, cx)
        })
        .into_any_element()
}

/// A lead column wide enough for the longest number in a list of `count`,
/// measured in the face the numbers are drawn in.
pub fn lead_width(count: usize, window: &Window) -> Pixels {
    let digits = count.max(1).to_string().len().max(2) as f32;
    let text = window.text_system();
    let font = text.resolve_font(&font(tokens::FONT));
    let digit = text
        .advance(font, Type::Meta.size(), '0')
        .map(|size| size.width)
        .unwrap_or(Type::Meta.size() * 0.6);
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
