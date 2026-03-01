use super::state::{PlaylistItem, QueueItemId};

/// Maximum number of undo entries retained.
const MAX_UNDO_DEPTH: usize = 100;

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

    /// Playlist was cleared / replaced. Undo = restore this snapshot.
    Replaced {
        items: Vec<PlaylistItem>,
        cursor: Option<QueueItemId>,
    },
}

/// Standard undo/redo stack with bounded depth.
///
/// Lives on the Player struct — single-threaded, only the player command loop
/// touches it. New actions clear the redo stack (standard semantics).
#[derive(Debug, Default)]
pub struct UndoStack {
    undo: Vec<UndoEntry>,
    redo: Vec<UndoEntry>,
}

impl UndoStack {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push an undo entry. Clears the redo stack.
    pub fn push(&mut self, entry: UndoEntry) {
        self.redo.clear();
        self.undo.push(entry);
        if self.undo.len() > MAX_UNDO_DEPTH {
            self.undo.remove(0);
        }
    }

    /// Pop the most recent undo entry (for Ctrl+Z).
    pub fn pop_undo(&mut self) -> Option<UndoEntry> {
        self.undo.pop()
    }

    /// Pop the most recent redo entry (for Ctrl+Y / Ctrl+Shift+Z).
    pub fn pop_redo(&mut self) -> Option<UndoEntry> {
        self.redo.pop()
    }

    /// Push an entry onto the redo stack (called when undoing).
    pub fn push_redo(&mut self, entry: UndoEntry) {
        self.redo.push(entry);
    }

    /// Push an entry onto the undo stack without clearing redo
    /// (called when redoing).
    pub fn push_undo_keep_redo(&mut self, entry: UndoEntry) {
        self.undo.push(entry);
        if self.undo.len() > MAX_UNDO_DEPTH {
            self.undo.remove(0);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_id() -> QueueItemId {
        QueueItemId::new()
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
