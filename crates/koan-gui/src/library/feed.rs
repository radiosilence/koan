//! The engine's state as the library pages need it: the library's version,
//! the record and track playing, the downloads still arriving by record, and
//! the favourites. A slice of the engine mirror, kept here until the app's
//! own mirror replaces it.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui_kit::*;
use koan_ffi::{KoanEngine, PlayState, StateSlice, TransferState};

use super::drag::Playable;

pub struct Feed {
    pub engine: Arc<KoanEngine>,
    /// Moves whenever the library's rows do. Pages showing a listing ask for
    /// it again. `None` until the engine's first report.
    pub library_version: Option<u64>,
    pub playing_album: Option<i64>,
    pub playing_track: Option<i64>,
    /// The track under the cursor is playing rather than paused or waiting.
    pub playing: bool,
    /// The tracks still arriving, by record, for a tile's download bar.
    pub arriving: HashMap<i64, Vec<i64>>,
    /// Each track with a transfer, and where it is: a row's availability.
    pub transfers: HashMap<i64, TransferState>,
    /// Bumped whenever the connection to the server changes: artwork that
    /// failed to arrive is worth asking for again.
    pub connection_epoch: u64,
    /// Which server, and whether the link to it is up: what the epoch
    /// follows. The rest of the connection report (sharing, devices) moves
    /// for reasons artwork does not care about.
    reach: Option<(Option<String>, bool, Option<String>)>,
    /// The playlists a pick can be added to: every one not read-only.
    pub playlists: Vec<(i64, SharedString)>,
    favourites: Favourites,
    /// One read at a time; one asked for while another runs makes it read
    /// again when it lands.
    favourites_reading: bool,
    favourites_stale: bool,
    _observe: Task<()>,
}

impl Feed {
    pub fn new(engine: Arc<KoanEngine>, cx: &mut Context<Self>) -> Self {
        let stream = engine.observe();
        let observe = cx.spawn(async move |this, cx| {
            while let Some(batch) = stream.next().await {
                if this.update(cx, |feed, cx| feed.apply(batch, cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            engine,
            library_version: None,
            playing_album: None,
            playing_track: None,
            playing: false,
            arriving: HashMap::new(),
            transfers: HashMap::new(),
            connection_epoch: 0,
            reach: None,
            playlists: Vec::new(),
            favourites: Favourites::default(),
            favourites_reading: false,
            favourites_stale: false,
            _observe: observe,
        }
    }

    fn apply(&mut self, batch: Vec<StateSlice>, cx: &mut Context<Self>) {
        let mut changed = false;
        for slice in batch {
            match slice {
                StateSlice::Library { version } => {
                    if Some(version) != self.library_version {
                        self.library_version = Some(version);
                        self.reload_favourites(cx);
                        self.reload_playlists(cx);
                        changed = true;
                    }
                }
                StateSlice::Playback { now_playing } => {
                    let entry = now_playing.entry.as_ref();
                    let album = entry.and_then(|e| e.album_id);
                    let track = entry.and_then(|e| e.track_id);
                    let playing = now_playing.state == PlayState::Playing && !now_playing.waiting;
                    if (album, track, playing)
                        != (self.playing_album, self.playing_track, self.playing)
                    {
                        self.playing_album = album;
                        self.playing_track = track;
                        self.playing = playing;
                        changed = true;
                    }
                }
                StateSlice::Transfers { transfers } => {
                    let mut arriving: HashMap<i64, Vec<i64>> = HashMap::new();
                    let by_track: HashMap<i64, TransferState> =
                        transfers.iter().map(|t| (t.track_id, t.state)).collect();
                    if by_track != self.transfers {
                        self.transfers = by_track;
                        changed = true;
                    }
                    for t in transfers {
                        if matches!(t.state, TransferState::Queued | TransferState::Running)
                            && let Some(album) = t.album_id
                        {
                            arriving.entry(album).or_default().push(t.track_id);
                        }
                    }
                    if arriving != self.arriving {
                        self.arriving = arriving;
                        changed = true;
                    }
                }
                StateSlice::Connection { connection } => {
                    let reach = Some((
                        connection.signed_in_to,
                        connection.linked,
                        connection.server_kind,
                    ));
                    if reach != self.reach {
                        self.reach = reach;
                        self.connection_epoch += 1;
                        changed = true;
                    }
                }
                _ => {}
            }
        }
        if changed {
            cx.notify();
        }
    }

    pub fn is_favourite(&self, item: Playable) -> bool {
        self.favourites.contains(item)
    }

    /// Flips at once, then takes the engine's answer, and reads every id
    /// again after it, as the Mac does.
    pub fn toggle_favourite(&mut self, item: Playable, cx: &mut Context<Self>) {
        let on = self.favourites.flip(item);
        cx.notify();
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let answer = match item {
                Playable::Track(id) => engine.toggle_favourite(id).await,
                Playable::Album(id) => engine.toggle_favourite_album(id).await,
                Playable::Artist(id) => engine.toggle_favourite_artist(id).await,
            };
            this.update(cx, |feed, cx| {
                let now = answer
                    .inspect_err(|e| log::warn!("favourite {item:?}: {e}"))
                    .unwrap_or(!on);
                feed.favourites.answered(item, now);
                feed.reload_favourites(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn reload_playlists(&mut self, cx: &mut Context<Self>) {
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let playlists = match engine.playlists().await {
                Ok(p) => p,
                Err(e) => {
                    log::warn!("playlists: {e}");
                    return;
                }
            };
            this.update(cx, |feed, cx| {
                feed.playlists = playlists
                    .into_iter()
                    .filter(|p| !p.readonly)
                    .map(|p| (p.id, p.name.into()))
                    .collect();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Every favourite id, read again: a sync or another device can change
    /// them without this app toggling anything. A read that a toggle or a
    /// newer library change overtook is not applied but read again, since
    /// neither is sure to be followed by another change.
    fn reload_favourites(&mut self, cx: &mut Context<Self>) {
        if self.favourites_reading {
            self.favourites_stale = true;
            return;
        }
        self.favourites_reading = true;
        self.favourites_stale = false;
        let engine = self.engine.clone();
        let edits = self.favourites.edits;
        cx.spawn(async move |this, cx| {
            let read = async {
                Ok::<_, koan_ffi::KoanError>((
                    engine.clone().favourite_track_ids().await?,
                    engine.clone().favourite_album_ids().await?,
                    engine.favourite_artist_ids().await?,
                ))
            }
            .await;
            this.update(cx, |feed, cx| {
                feed.favourites_reading = false;
                let (tracks, albums, artists) = match read {
                    Ok(read) => read,
                    // A read asked for meanwhile still runs, once: its
                    // failing too ends it, since that read clears the flag.
                    Err(e) => {
                        log::warn!("favourites: {e}");
                        if feed.favourites_stale {
                            feed.reload_favourites(cx);
                        }
                        return;
                    }
                };
                let read = tracks
                    .into_iter()
                    .map(Playable::Track)
                    .chain(albums.into_iter().map(Playable::Album))
                    .chain(artists.into_iter().map(Playable::Artist))
                    .collect();
                if feed.favourites_stale || !feed.favourites.read_landed(edits, read) {
                    feed.reload_favourites(cx);
                    return;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// The favourite ids, with the hearts clicked whose answer has not come.
///
/// A read of every id can start before a toggle is written and land after
/// the engine has answered it, carrying the old state. So `edits` moves
/// when a heart is clicked and again when its answer lands, and a read that
/// either overtook is not applied; a read that lands while a toggle is still
/// out keeps that toggle's flip over it.
#[derive(Default)]
struct Favourites {
    set: HashSet<Playable>,
    pending: HashMap<Playable, bool>,
    edits: u64,
}

impl Favourites {
    fn contains(&self, item: Playable) -> bool {
        self.set.contains(&item)
    }

    fn put(&mut self, item: Playable, on: bool) {
        if on {
            self.set.insert(item);
        } else {
            self.set.remove(&item);
        }
    }

    /// A heart clicked: shown flipped at once. Returns the state it shows.
    fn flip(&mut self, item: Playable) -> bool {
        let on = !self.contains(item);
        self.put(item, on);
        self.pending.insert(item, on);
        self.edits += 1;
        on
    }

    /// The engine's answer: the state it wrote, or the state before the
    /// click when the write failed.
    fn answered(&mut self, item: Playable, now: bool) {
        self.pending.remove(&item);
        self.put(item, now);
        self.edits += 1;
    }

    /// A read that started at `edits` landing. Whether it was applied.
    fn read_landed(&mut self, edits: u64, read: HashSet<Playable>) -> bool {
        if edits != self.edits {
            return false;
        }
        self.set = read;
        let pending: Vec<_> = self.pending.iter().map(|(i, on)| (*i, *on)).collect();
        for (item, on) in pending {
            self.put(item, on);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    const ARTIST: Playable = Playable::Artist(7);
    const TRACK: Playable = Playable::Track(3);

    #[test]
    fn a_read_that_started_before_the_answer_cannot_undo_it() {
        let mut f = Favourites::default();
        assert!(f.flip(ARTIST));
        let started = f.edits;
        f.answered(ARTIST, true);
        // Read before the write, landing after the answer.
        assert!(!f.read_landed(started, HashSet::new()));
        assert!(f.contains(ARTIST));
    }

    #[test]
    fn a_read_while_the_toggle_is_out_keeps_its_flip() {
        let mut f = Favourites::default();
        f.flip(TRACK);
        let started = f.edits;
        assert!(f.read_landed(started, HashSet::new()));
        assert!(f.contains(TRACK));
        f.answered(TRACK, true);
        let started = f.edits;
        assert!(f.read_landed(started, [TRACK].into()));
        assert!(f.contains(TRACK));
    }

    #[test]
    fn a_failed_toggle_goes_back_and_a_later_read_is_taken() {
        let mut f = Favourites::default();
        let on = f.flip(TRACK);
        f.answered(TRACK, !on);
        assert!(!f.contains(TRACK));
        let started = f.edits;
        assert!(f.read_landed(started, [ARTIST].into()));
        assert!(f.contains(ARTIST) && !f.contains(TRACK));
    }
}
