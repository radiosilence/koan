//! A record's cover, as large as the window allows, over everything: what a
//! click on the sleeve of a record's page or the transport opens. It belongs
//! to what was being looked at, so it is a layer over the window rather than
//! a window, and a click anywhere or Escape closes it.
//!
//! The picture is decoded at the size it is drawn at, rounded up to a step of
//! 256 points, off the main thread,
//! by the sleeve cache every view shares; until it lands the record's
//! placeholder holds its place.

use koan_kit::app::*;

use crate::components::Viewer;
use crate::covers::{self, CoversHandle};
use crate::theme::Look;

struct Showing {
    album: i64,
    title: SharedString,
    detail: Option<SharedString>,
    /// Where the sleeve was tapped: the picture opens out of it.
    from: Option<Point<Pixels>>,
}

/// What the viewer shows, while it shows.
#[derive(Default)]
pub struct Artwork {
    showing: Option<Showing>,
    focus: Option<FocusHandle>,
    /// What held the keyboard before the viewer took it, given it back on
    /// closing.
    before: Option<FocusHandle>,
}

struct ArtworkHandle(Entity<Artwork>);

impl Global for ArtworkHandle {}

/// The viewer's state, made the first time it is wanted. Observed by the
/// shell, which draws the viewer over the window.
pub fn artwork(cx: &mut App) -> Entity<Artwork> {
    if let Some(handle) = cx.try_global::<ArtworkHandle>() {
        return handle.0.clone();
    }
    let entity = cx.new(|_| Artwork::default());
    cx.set_global(ArtworkHandle(entity.clone()));
    entity
}

/// Opens the viewer on `album`'s cover, taking the keyboard so Escape
/// closes it.
pub fn show(
    album: i64,
    title: impl Into<SharedString>,
    detail: Option<SharedString>,
    window: &mut Window,
    cx: &mut App,
) {
    let before = window.focused(cx);
    let focus = cx.focus_handle();
    window.focus(&focus, cx);
    let (title, detail) = (title.into(), detail);
    let from = Look::of(window, cx).touch.then(|| window.mouse_position());
    artwork(cx).update(cx, |a, cx| {
        a.showing = Some(Showing {
            album,
            title,
            detail,
            from,
        });
        a.focus = Some(focus);
        // Opened again over itself, it keeps what it first took over.
        if a.before.is_none() {
            a.before = before;
        }
        cx.notify();
    });
}

fn hide(window: &mut Window, cx: &mut App) {
    let before = artwork(cx).update(cx, |a, cx| {
        a.showing.take()?;
        a.focus = None;
        cx.notify();
        a.before.take()
    });
    if let Some(before) = before {
        window.focus(&before, cx);
    }
}

/// Whether the viewer shows: the shell redraws for a sleeve landing only
/// then.
pub fn is_showing(cx: &mut App) -> bool {
    artwork(cx).read(cx).showing.is_some()
}

/// The sleeve cache the viewer's picture lands in, for the shell to observe.
pub fn sleeves(cx: &App) -> Entity<covers::Covers> {
    cx.global::<CoversHandle>().0.clone()
}

/// The viewer, for the shell to draw over the window while it shows.
pub fn overlay(window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let entity = artwork(cx);
    let (album, title, detail, from, focus) = {
        let a = entity.read(cx);
        let s = a.showing.as_ref()?;
        (
            s.album,
            s.title.clone(),
            s.detail.clone(),
            s.from,
            a.focus.clone(),
        )
    };
    let look = Look::of(window, cx);
    // Decoded in steps of 256 points, so a window being resized asks for a
    // new size, and a new decode, only now and then; the viewer scales it.
    let side = Viewer::side(window.viewport_size(), &look);
    let decoded = Pixels::from((f32::from(side) / 256.0).ceil() * 256.0);
    let art = covers::art(Some(album), decoded, window.scale_factor(), cx);
    let mut viewer = Viewer::new("artwork-viewer", art, title)
        .from_point(from)
        .on_click(|_, window, cx| hide(window, cx));
    if let Some(detail) = detail {
        viewer = viewer.detail(detail);
    }
    Some(
        div()
            .absolute()
            .inset_0()
            .when_some(focus, |d, focus| d.track_focus(&focus))
            .on_key_down(|e: &KeyDownEvent, window, cx| {
                if e.keystroke.key == "escape" {
                    hide(window, cx);
                }
            })
            .child(viewer)
            .into_any_element(),
    )
}
