//! What a drag out of the library carries: playable things, named by id, in
//! the order they were picked. The queue and the playlists accept it and hand
//! it to the engine as `TrackSet`s, which resolve to tracks on the engine's
//! command lane, so a drop never waits on reading a record's tracks first.

use koan_kit::app::*;
use std::sync::Arc;

use koan_runtime::TrackSet;

use super::tokens::Paint;
use crate::components::DragChip;

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
    /// The record whose sleeve the preview shows: the first record dragged,
    /// or the record of the tracks dragged when the page knows it.
    pub sleeve: Option<i64>,
    /// The playlist rows it was dragged out of. Dropped back on that
    /// playlist it moves them rather than adding copies; on that playlist's
    /// sidebar row, nothing.
    pub origin: Option<Origin>,
}

/// Rows of a playlist, by entry: one track can be several of them.
#[derive(Clone, Debug)]
pub struct Origin {
    pub playlist: i64,
    pub entries: Arc<[i64]>,
}

impl PlayableDrag {
    pub fn new(items: &[Playable], label: impl Into<SharedString>) -> Self {
        Self {
            sets: sources(items).into(),
            items: items.into(),
            label: label.into(),
            sleeve: items.iter().find_map(|item| match item {
                Playable::Album(id) => Some(*id),
                _ => None,
            }),
            origin: None,
        }
    }

    /// Marks it as rows of `playlist`.
    pub fn from_playlist(mut self, playlist: i64, entries: Vec<i64>) -> Self {
        self.origin = Some(Origin {
            playlist,
            entries: entries.into(),
        });
        self
    }

    /// Whether it came out of `playlist`.
    pub fn is_from(&self, playlist: i64) -> bool {
        self.origin.as_ref().is_some_and(|o| o.playlist == playlist)
    }

    /// The record whose sleeve the preview shows, where the first item is
    /// not a record itself.
    pub fn with_sleeve(mut self, album: Option<i64>) -> Self {
        self.sleeve = self.sleeve.or(album);
        self
    }

    /// The drag preview, centred under the pointer.
    pub fn preview(&self, at: Point<Pixels>, cx: &mut App) -> Entity<DragPreview> {
        let (label, sleeve) = (self.label.clone(), self.sleeve);
        cx.new(|_| DragPreview { label, sleeve, at })
    }
}

/// A playlist dragged from the sidebar: dropped among the playlists it moves
/// there, dropped on the queue its tracks are queued.
#[derive(Clone, Debug)]
pub struct PlaylistDrag {
    pub id: i64,
    pub name: SharedString,
    /// The record whose sleeve the preview shows: the playlist's first.
    pub sleeve: Option<i64>,
}

impl PlaylistDrag {
    /// What the queue takes from it: the playlist's tracks, as one set.
    pub fn as_library(&self) -> PlayableDrag {
        PlayableDrag {
            sets: vec![TrackSet::Playlist { id: self.id }].into(),
            items: Vec::new().into(),
            label: self.name.clone(),
            sleeve: self.sleeve,
            origin: None,
        }
    }

    pub fn preview(&self, at: Point<Pixels>, cx: &mut App) -> Entity<DragPreview> {
        self.as_library().preview(at, cx)
    }
}

pub struct DragPreview {
    label: SharedString,
    sleeve: Option<i64>,
    at: Point<Pixels>,
}

impl Render for DragPreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &Paint::of(window, cx);
        let side = crate::components::RowMetrics::nav_art(&p.look);
        let mut chip = DragChip::new(self.label.clone());
        if self.sleeve.is_some() {
            chip = chip.sleeve(crate::covers::art(
                self.sleeve,
                side,
                window.scale_factor(),
                cx,
            ));
        }
        div().pl(self.at.x).pt(self.at.y).child(chip)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

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
