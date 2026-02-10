use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;

/// Thread-safe track queue. Shared between the player thread and decode thread.
///
/// The player pushes tracks in; the decode thread pops from the front when
/// the current track hits EOF (gapless transition).
#[derive(Debug)]
pub struct TrackQueue {
    tracks: Mutex<VecDeque<PathBuf>>,
}

impl Default for TrackQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl TrackQueue {
    pub fn new() -> Self {
        Self {
            tracks: Mutex::new(VecDeque::new()),
        }
    }

    /// Replace the entire queue with new tracks.
    pub fn set(&self, tracks: Vec<PathBuf>) {
        let mut q = self.tracks.lock().unwrap();
        q.clear();
        q.extend(tracks);
    }

    /// Pop the next track from the front.
    pub fn pop_front(&self) -> Option<PathBuf> {
        self.tracks.lock().unwrap().pop_front()
    }

    /// Push a track to the back.
    pub fn push_back(&self, path: PathBuf) {
        self.tracks.lock().unwrap().push_back(path);
    }

    /// Push a track to the front (for going back).
    pub fn push_front(&self, path: PathBuf) {
        self.tracks.lock().unwrap().push_front(path);
    }

    /// Clear the queue.
    pub fn clear(&self) {
        self.tracks.lock().unwrap().clear();
    }

    /// Number of tracks remaining.
    pub fn len(&self) -> usize {
        self.tracks.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clone all queued paths for UI display. Non-destructive.
    pub fn snapshot(&self) -> Vec<PathBuf> {
        self.tracks.lock().unwrap().iter().cloned().collect()
    }

    /// Remove the track at `index`. Returns it if valid.
    pub fn remove(&self, index: usize) -> Option<PathBuf> {
        self.tracks.lock().unwrap().remove(index)
    }

    /// Move a track from one position to another.
    pub fn move_track(&self, from: usize, to: usize) {
        let mut q = self.tracks.lock().unwrap();
        if from < q.len()
            && to < q.len()
            && let Some(track) = q.remove(from)
        {
            q.insert(to, track);
        }
    }
}
