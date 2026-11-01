//! What a drag out of the library carries: playable things, named by id, in
//! the order they were picked. The queue and the playlists accept it and hand
//! it to the engine as `TrackSet`s, which resolve to tracks on the engine's
//! command lane, so a drop never waits on reading a record's tracks first.

use koan_kit::app::*;
use std::sync::Arc;

use koan_ffi::TrackSet;

use super::tokens::{Paint, Type, Typeset};
use crate::theme::Space;

/// One thing a page shows that can be played.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Playable {
    Track(i64),
    Album(i64),
    Artist(i64),
}

impl Playable {
    pub fn source(self) -> TrackSet {
        match self {
            Playable::Track(id) => TrackSet::Tracks { ids: vec![id] },
            Playable::Album(id) => TrackSet::Album { id },
            Playable::Artist(id) => TrackSet::Artist { id },
        }
    }
}

/// What the engine's queue and playlist calls take for `items`, in order.
/// Neighbouring tracks merge into one set, so two hundred ticked tracks are
/// one source rather than two hundred.
pub fn sources(items: &[Playable]) -> Vec<TrackSet> {
    let mut out: Vec<TrackSet> = Vec::with_capacity(items.len());
    for item in items {
        match (item, out.last_mut()) {
            (Playable::Track(id), Some(TrackSet::Tracks { ids })) => ids.push(*id),
            (item, _) => out.push(item.source()),
        }
    }
    out
}

/// The payload of every drag out of a library page. A queue or playlist that
/// accepts it hands `sets` to the engine as they are. Shared, so a pick of
/// thousands is built once and every tile in it hands out the same one.
#[derive(Clone, Debug)]
pub struct PlayableDrag {
    pub sets: Arc<[TrackSet]>,
    /// What was picked, in order: what a context menu over the drag acts on.
    pub items: Arc<[Playable]>,
    /// What the preview says: the one item's name, or how many.
    pub label: SharedString,
}

impl PlayableDrag {
    pub fn new(items: &[Playable], label: impl Into<SharedString>) -> Self {
        Self {
            sets: sources(items).into(),
            items: items.into(),
            label: label.into(),
        }
    }

    /// The drag preview, centred under the pointer.
    pub fn preview(&self, at: Point<Pixels>, cx: &mut App) -> Entity<DragPreview> {
        let label = self.label.clone();
        cx.new(|_| DragPreview { label, at })
    }
}

pub struct DragPreview {
    label: SharedString,
    at: Point<Pixels>,
}

impl Render for DragPreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &Paint::of(window, cx);
        div().pl(self.at.x).pt(self.at.y).child(
            div()
                .px(p.space(Space::M))
                .py(p.space(Space::Xs))
                .bg(p.bg)
                .border_1()
                .border_color(p.rule)
                .typeset(p, Type::Meta, p.ink)
                .whitespace_nowrap()
                .child(self.label.clone()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neighbouring_tracks_merge_into_one_source() {
        let drag = PlayableDrag::new(
            &[
                Playable::Track(1),
                Playable::Track(2),
                Playable::Album(9),
                Playable::Track(3),
            ],
            "4 items",
        );
        let sources = drag.sets.to_vec();
        assert_eq!(sources.len(), 3);
        assert!(matches!(&sources[0], TrackSet::Tracks { ids } if ids == &[1, 2]));
        assert!(matches!(&sources[1], TrackSet::Album { id: 9 }));
        assert!(matches!(&sources[2], TrackSet::Tracks { ids } if ids == &[3]));
    }
}
