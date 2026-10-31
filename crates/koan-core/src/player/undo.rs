use std::collections::VecDeque;

use super::state::{PlaylistItem, QueueItemId};

/// Maximum number of undo entries retained.
const MAX_UNDO_DEPTH: usize = 100;

/// Maximum number of queue rows the entries of one stack hold between them,
/// at roughly half a kilobyte a row. A replaced queue is kept whole, so depth
/// alone lets a hundred replacements of a large library sit in memory. The
/// newest entry is always kept.
const MAX_UNDO_ROWS: usize = 50_000;

/// Ids counted as one row: an entry that names rows by id holds a small
/// fraction of what one holding the rows does.
const IDS_PER_ROW: usize = 16;

/// A reversible playlist operation. Stores enough state to undo/redo.
///
/// Each variant describes an action to reverse. `apply_entry` executes the
/// reversal and returns the inverse entry for the opposite stack.
#[derive(Debug, Clone)]
pub enum UndoEntry {
    /// Items were added (append). Undo = remove by IDs.
    Added { ids: Vec<QueueItemId> },

    /// Items were removed. Undo = re-add them at their original positions.
    /// Each tuple: (item, id-of-predecessor or None if was first).
    Removed {
        items: Vec<(Box<PlaylistItem>, Option<QueueItemId>)>,
    },

    /// Items were inserted after a specific item. Undo = remove by IDs.
    Inserted { ids: Vec<QueueItemId> },

    /// A single item was moved. Undo = move it back.
    Moved {
        id: QueueItemId,
        was_after: Option<QueueItemId>,
    },

    /// Multiple items were moved. Undo = restore original positions.
    MovedBatch {
        entries: Vec<(QueueItemId, Option<QueueItemId>)>,
    },

    /// The rows after the cursor were shuffled in place. Undone as
    /// `MovedBatch` is; kept apart so the shuffles on top of the stack can be
    /// counted.
    Shuffled {
        entries: Vec<(QueueItemId, Option<QueueItemId>)>,
    },

    /// Playlist was cleared / replaced. Undo = restore this snapshot.
    Replaced {
        items: Vec<PlaylistItem>,
        cursor: Option<QueueItemId>,
    },

    /// Multiple operations batched as a single undo step (e.g. drag reorder).
    Batch(Vec<UndoEntry>),
}

impl UndoEntry {
    /// How many queue rows' worth of memory the entry holds.
    fn rows(&self) -> usize {
        match self {
            UndoEntry::Added { ids } | UndoEntry::Inserted { ids } => {
                ids.len().div_ceil(IDS_PER_ROW)
            }
            UndoEntry::Removed { items } => items.len(),
            UndoEntry::Moved { .. } => 1,
            UndoEntry::MovedBatch { entries } | UndoEntry::Shuffled { entries } => {
                entries.len().div_ceil(IDS_PER_ROW)
            }
            UndoEntry::Replaced { items, .. } => items.len(),
            UndoEntry::Batch(entries) => entries.iter().map(UndoEntry::rows).sum(),
        }
    }
}

/// Push onto one side, dropping the oldest entries past the bounds.
fn bounded_push(stack: &mut VecDeque<UndoEntry>, entry: UndoEntry) {
    stack.push_back(entry);
    let mut rows: usize = stack.iter().map(UndoEntry::rows).sum();
    while stack.len() > MAX_UNDO_DEPTH || (rows > MAX_UNDO_ROWS && stack.len() > 1) {
        if let Some(dropped) = stack.pop_front() {
            rows -= dropped.rows();
        }
    }
}

/// Standard undo/redo stack, bounded by depth and by the rows it holds.
///
/// Lives on the Player struct — single-threaded, only the player command loop
/// touches it. New actions clear the redo stack (standard semantics).
///
/// Uses `VecDeque` so evicting the oldest entry (front) is O(1) instead of
/// the O(n) `Vec::remove(0)`.
#[derive(Debug, Default)]
pub struct UndoStack {
    undo: VecDeque<UndoEntry>,
    redo: VecDeque<UndoEntry>,
}

impl UndoStack {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push an undo entry. Clears the redo stack.
    pub fn push(&mut self, entry: UndoEntry) {
        self.redo.clear();
        bounded_push(&mut self.undo, entry);
    }

    /// Pop the most recent undo entry (for Ctrl+Z).
    pub fn pop_undo(&mut self) -> Option<UndoEntry> {
        self.undo.pop_back()
    }

    /// Pop the most recent redo entry (for Ctrl+Y / Ctrl+Shift+Z).
    pub fn pop_redo(&mut self) -> Option<UndoEntry> {
        self.redo.pop_back()
    }

    /// Push an entry onto the redo stack (called when undoing).
    pub fn push_redo(&mut self, entry: UndoEntry) {
        bounded_push(&mut self.redo, entry);
    }

    /// Push an entry onto the undo stack without clearing redo
    /// (called when redoing).
    pub fn push_undo_keep_redo(&mut self, entry: UndoEntry) {
        bounded_push(&mut self.undo, entry);
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// How many shuffles undo would take back before reaching anything else.
    pub fn shuffles_on_top(&self) -> u32 {
        self.undo
            .iter()
            .rev()
            .take_while(|e| matches!(e, UndoEntry::Shuffled { .. }))
            .count() as u32
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_id() -> QueueItemId {
        QueueItemId::new()
    }

    fn removed(rows: usize) -> UndoEntry {
        let item = PlaylistItem {
            uid: None,
            playlist_entry_id: None,
            id: dummy_id(),
            db_id: None,
            path: "/x.flac".into(),
            title: String::new(),
            artist: String::new(),
            album_artist: String::new(),
            album: String::new(),
            year: None,
            codec: None,
            track_number: None,
            disc: None,
            duration_ms: None,
            state: super::super::state::ItemState::Ready,
            played: false,
        };
        UndoEntry::Removed {
            items: (0..rows).map(|_| (Box::new(item.clone()), None)).collect(),
        }
    }

    #[test]
    fn large_entries_are_bounded_by_rows() {
        let mut stack = UndoStack::new();
        for _ in 0..10 {
            stack.push(removed(10_000));
        }
        assert_eq!(stack.undo_len(), MAX_UNDO_ROWS / 10_000);
        stack.push(removed(MAX_UNDO_ROWS * 2));
        assert_eq!(
            stack.undo_len(),
            1,
            "the newest entry is kept whatever its size"
        );
    }

    #[test]
    fn entries_of_ids_weigh_less_than_entries_of_rows() {
        let mut stack = UndoStack::new();
        stack.push(removed(MAX_UNDO_ROWS / 2));
        stack.push(UndoEntry::Added {
            ids: (0..MAX_UNDO_ROWS).map(|_| dummy_id()).collect(),
        });
        assert_eq!(stack.undo_len(), 2);
    }

    #[test]
    fn push_and_pop_undo() {
        let mut stack = UndoStack::new();
        let id = dummy_id();
        stack.push(UndoEntry::Added { ids: vec![id] });
        assert!(stack.can_undo());
        assert!(!stack.can_redo());

        let entry = stack.pop_undo().unwrap();
        assert!(matches!(entry, UndoEntry::Added { .. }));
        assert!(!stack.can_undo());
    }

    #[test]
    fn new_action_clears_redo() {
        let mut stack = UndoStack::new();
        let id = dummy_id();
        stack.push(UndoEntry::Added { ids: vec![id] });
        let entry = stack.pop_undo().unwrap();
        stack.push_redo(entry);
        assert!(stack.can_redo());

        // New action should clear redo
        stack.push(UndoEntry::Added { ids: vec![id] });
        assert!(!stack.can_redo());
    }

    #[test]
    fn max_depth_enforced() {
        let mut stack = UndoStack::new();
        for _ in 0..150 {
            stack.push(UndoEntry::Added {
                ids: vec![dummy_id()],
            });
        }
        assert_eq!(stack.undo_len(), MAX_UNDO_DEPTH);
    }

    #[test]
    fn push_undo_keep_redo_preserves_redo() {
        let mut stack = UndoStack::new();
        let id = dummy_id();
        stack.push_redo(UndoEntry::Added { ids: vec![id] });
        assert!(stack.can_redo());

        stack.push_undo_keep_redo(UndoEntry::Added { ids: vec![id] });
        assert!(stack.can_undo());
        assert!(stack.can_redo()); // redo NOT cleared
    }
}
