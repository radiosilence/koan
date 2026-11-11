//! The engine's state as the library pages need it: the library's version,
//! the record and track playing, the downloads still arriving by record, and
//! the favourites. Derived from the app's mirror (`crate::mirror`), whose
//! slices it follows, reshaped once for every page; the favourites and
//! playlists, which no slice carries, it reads from the engine as the
//! library moves.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use koan_kit::app::*;
use koan_runtime::{KoanEngine, TransferState};

use super::drag::Playable;

/// Moves whenever a favourite is toggled from the app: every page's feed and
/// the Dock's menu read the favourites again, so a heart and the Dock's
/// Favourite check agree. A library change moves them as well, through the
/// library slice.
#[derive(Default)]
pub struct FavouritesMoved(u64);

impl Global for FavouritesMoved {}

/// A favourite was toggled and the engine has answered.
pub fn favourites_moved(cx: &mut App) {
    cx.default_global::<FavouritesMoved>().0 += 1;
}

pub struct Feed {
    pub engine: Arc<KoanEngine>,
    /// Moves whenever the library's rows do. Pages showing a listing ask for
    /// it again. `None` until the engine's first report.
    pub library_version: Option<u64>,
    /// Moves whenever the play history does: what Recently played follows.
    pub history_version: Option<u64>,
    /// Moves whenever the favourites the engine holds may have changed (a
    /// heart's answer, a read of every id): what Favourites follows.
    pub favourites_revision: u64,
    pub playing_album: Option<i64>,
    pub playing_track: Option<i64>,
    /// The tracks still arriving, by record, for a tile's download bar.
    pub arriving: HashMap<i64, Vec<i64>>,
    /// Each track with a transfer, and where it is: a row's availability.
    pub transfers: HashMap<i64, TransferState>,
    /// Bumped whenever the connection to the server changes: artwork that
    /// failed to arrive is worth asking for again.
    pub connection_epoch: u64,
    /// Nothing can be fetched now (`ConnectionInfo::out_of_reach`): a track
    /// not here reads as not on this device.
    pub out_of_reach: bool,
    /// Which server, and whether the link to it is up: what the epoch
    /// follows. The rest of the connection report (sharing, devices) moves
    /// for reasons artwork does not care about.
    reach: Option<(Option<String>, bool, Option<String>)>,
    /// The playlists a pick can be added to: every one not read-only.
    pub playlists: Vec<(i64, SharedString)>,
    /// Every playlist, read-only ones included: what a list of them shows.
    pub all_playlists: Vec<(i64, SharedString)>,
    /// The newest read of the playlists asked for: an older one landing
    /// after it is dropped.
    playlists_read: u64,
    favourites: Favourites,
    /// One read at a time; one asked for while another runs makes it read
    /// again when it lands.
    favourites_reading: bool,
    favourites_stale: bool,
    _observe: [Subscription; 6],
}

impl Feed {
    pub fn new(engine: Arc<KoanEngine>, cx: &mut Context<Self>) -> Self {
        let mirror = cx.global::<crate::mirror::Mirror>().clone();
        let observe = [
            cx.observe(&mirror.library, |feed, library, cx| {
                let library = library.read(cx);
                let version = library.known.then_some(library.version);
                feed.library(version, cx);
            }),
            cx.observe(&mirror.playback, |feed, playback, cx| {
                let entry = playback.read(cx).0.entry.as_ref();
                let playing = (
                    entry.and_then(|e| e.album_id),
                    entry.and_then(|e| e.track_id),
                );
                feed.playback(playing, cx);
            }),
            cx.observe(&mirror.transfers, |feed, transfers, cx| {
                let derived = Self::by_track(&transfers.read(cx).0);
                feed.transfers(derived, cx);
            }),
            cx.observe(&mirror.connection, |feed, connection, cx| {
                let connection = &connection.read(cx).0;
                let out = connection.as_ref().is_some_and(|c| c.out_of_reach);
                if out != feed.out_of_reach {
                    feed.out_of_reach = out;
                    cx.notify();
                }
                let reach = Self::reach(connection);
                feed.connection(reach, cx);
            }),
            // A favourite toggled anywhere in the app: the Dock's menu, or
            // another window's page.
            cx.observe_global::<FavouritesMoved>(|feed, cx| feed.reload_favourites(cx)),
            cx.observe(&mirror.history, |feed, history, cx| {
                let version = Some(history.read(cx).0);
                if version != feed.history_version {
                    feed.history_version = version;
                    cx.notify();
                }
            }),
        ];
        let mut feed = Self {
            engine,
            library_version: None,
            history_version: Some(mirror.history.read(cx).0),
            favourites_revision: 0,
            playing_album: None,
            playing_track: None,
            arriving: HashMap::new(),
            transfers: HashMap::new(),
            connection_epoch: 0,
            out_of_reach: false,
            reach: None,
            playlists: Vec::new(),
            all_playlists: Vec::new(),
            playlists_read: 0,
            favourites: Favourites::default(),
            favourites_reading: false,
            favourites_stale: false,
            _observe: observe,
        };
        // Whatever the mirror already holds, at once: the pages are made
        // after its first report has landed.
        let library = mirror.library.read(cx);
        feed.library(library.known.then_some(library.version), cx);
        let entry = mirror.playback.read(cx).0.entry.as_ref();
        let playing = (
            entry.and_then(|e| e.album_id),
            entry.and_then(|e| e.track_id),
        );
        feed.playback(playing, cx);
        let derived = Self::by_track(&mirror.transfers.read(cx).0);
        feed.transfers(derived, cx);
        // The connection as it stands is where artwork starts from, not a
        // change to retry failures for.
        feed.reach = Self::reach(&mirror.connection.read(cx).0);
        feed.out_of_reach = mirror
            .connection
            .read(cx)
            .0
            .as_ref()
            .is_some_and(|c| c.out_of_reach);
        feed
    }

    fn reach(
        connection: &Option<koan_runtime::ConnectionInfo>,
    ) -> Option<(Option<String>, bool, Option<String>)> {
        connection
            .as_ref()
            .map(|c| (c.signed_in_to.clone(), c.linked, c.server_kind.clone()))
    }

    fn library(&mut self, version: Option<u64>, cx: &mut Context<Self>) {
        if version.is_some() && version != self.library_version {
            self.library_version = version;
            self.reload_favourites(cx);
            self.reload_playlists(cx);
            cx.notify();
        }
    }

    fn playback(&mut self, playing: (Option<i64>, Option<i64>), cx: &mut Context<Self>) {
        if playing != (self.playing_album, self.playing_track) {
            (self.playing_album, self.playing_track) = playing;
            cx.notify();
        }
    }

    /// Each track's transfer, and the tracks still arriving by record.
    #[allow(clippy::type_complexity)]
    fn by_track(
        transfers: &[koan_runtime::Transfer],
    ) -> (HashMap<i64, TransferState>, HashMap<i64, Vec<i64>>) {
        let by_track = transfers.iter().map(|t| (t.track_id, t.state)).collect();
        let mut arriving: HashMap<i64, Vec<i64>> = HashMap::new();
        for t in transfers {
            if matches!(t.state, TransferState::Queued | TransferState::Running)
                && let Some(album) = t.album_id
            {
                arriving.entry(album).or_default().push(t.track_id);
            }
        }
        (by_track, arriving)
    }

    #[allow(clippy::type_complexity)]
    fn transfers(
        &mut self,
        (by_track, arriving): (HashMap<i64, TransferState>, HashMap<i64, Vec<i64>>),
        cx: &mut Context<Self>,
    ) {
        if by_track != self.transfers || arriving != self.arriving {
            self.transfers = by_track;
            self.arriving = arriving;
            cx.notify();
        }
    }

    fn connection(
        &mut self,
        reach: Option<(Option<String>, bool, Option<String>)>,
        cx: &mut Context<Self>,
    ) {
        if reach.is_some() && reach != self.reach {
            self.reach = reach;
            self.connection_epoch += 1;
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
        if on {
            crate::rainbow::celebrate(cx);
        }
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
                // Read again once the last click on this heart is answered:
                // a sync or another device may have moved others since.
                if feed.favourites.answered(item, now) {
                    feed.favourites_revision += 1;
                    // Every feed and the Dock read again, this one included.
                    favourites_moved(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn reload_playlists(&mut self, cx: &mut Context<Self>) {
        let engine = self.engine.clone();
        self.playlists_read += 1;
        let read = self.playlists_read;
        cx.spawn(async move |this, cx| {
            let playlists = match engine.playlists().await {
                Ok(p) => p,
                Err(e) => {
                    log::warn!("playlists: {e}");
                    return;
                }
            };
            this.update(cx, |feed, cx| {
                if read != feed.playlists_read {
                    return;
                }
                feed.playlists = playlists
                    .iter()
                    .filter(|p| !p.readonly)
                    .map(|p| (p.id, p.name.clone().into()))
                    .collect();
                feed.all_playlists = playlists
                    .into_iter()
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
                Ok::<_, koan_runtime::KoanError>((
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
                let before = feed.favourites.set.clone();
                if feed.favourites_stale || !feed.favourites.read_landed(edits, read) {
                    feed.reload_favourites(cx);
                    return;
                }
                // The read after a toggle finds what the toggle's answer
                // already counted: the shelves read again only for a change.
                if feed.favourites.set != before {
                    feed.favourites_revision += 1;
                    cx.notify();
                }
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
    /// Hearts clicked and not yet answered: the state shown, and how many
    /// toggles are in flight. Only the last answer is applied, since each
    /// click toggles the engine again.
    pending: HashMap<Playable, (bool, u32)>,
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
        let pending = self.pending.entry(item).or_insert((on, 0));
        *pending = (on, pending.1 + 1);
        self.edits += 1;
        on
    }

    /// The engine's answer: the state it wrote, or the state before the
    /// click when the write failed. An earlier click's answer while a later
    /// one is in flight leaves the later click's state shown. Whether this
    /// was the last answer.
    fn answered(&mut self, item: Playable, now: bool) -> bool {
        let last = match self.pending.get_mut(&item) {
            Some((_, in_flight)) if *in_flight > 1 => {
                *in_flight -= 1;
                false
            }
            _ => true,
        };
        if last {
            self.pending.remove(&item);
            self.put(item, now);
        }
        self.edits += 1;
        last
    }

    /// A read that started at `edits` landing. Whether it was applied.
    fn read_landed(&mut self, edits: u64, read: HashSet<Playable>) -> bool {
        if edits != self.edits {
            return false;
        }
        self.set = read;
        let pending: Vec<_> = self.pending.iter().map(|(i, (on, _))| (*i, *on)).collect();
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
    fn two_quick_clicks_keep_the_second_until_its_answer() {
        let mut f = Favourites::default();
        assert!(f.flip(TRACK));
        assert!(!f.flip(TRACK));
        // The first click's answer: the engine wrote it on, but the second
        // click is still out, so the heart stays off.
        assert!(!f.answered(TRACK, true));
        assert!(!f.contains(TRACK));
        // A read landing between the answers keeps the second click's state.
        let started = f.edits;
        assert!(f.read_landed(started, [TRACK].into()));
        assert!(!f.contains(TRACK));
        assert!(f.answered(TRACK, false));
        assert!(!f.contains(TRACK));
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
