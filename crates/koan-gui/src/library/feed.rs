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
    favourites: HashSet<Playable>,
    /// Bumped by every toggle, so a re-read that started before one cannot
    /// put back what it changed.
    favourite_edits: u64,
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
            favourites: HashSet::new(),
            favourite_edits: 0,
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

    /// How many things are favourited: what a page listing them follows.
    pub fn favourite_count(&self) -> usize {
        self.favourites.len()
    }

    pub fn is_favourite(&self, item: Playable) -> bool {
        self.favourites.contains(&item)
    }

    /// Flips at once, then takes the engine's answer.
    pub fn toggle_favourite(&mut self, item: Playable, cx: &mut Context<Self>) {
        let on = !self.favourites.contains(&item);
        self.set_favourite(item, on);
        self.favourite_edits += 1;
        cx.notify();
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let answer = match item {
                Playable::Track(id) => engine.toggle_favourite(id).await,
                Playable::Album(id) => engine.toggle_favourite_album(id).await,
                Playable::Artist(id) => engine.toggle_favourite_artist(id).await,
            };
            this.update(cx, |feed, cx| match answer {
                Ok(now) => {
                    feed.set_favourite(item, now);
                    cx.notify();
                }
                Err(e) => {
                    log::warn!("favourite {item:?}: {e}");
                    feed.set_favourite(item, !on);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn set_favourite(&mut self, item: Playable, on: bool) {
        if on {
            self.favourites.insert(item);
        } else {
            self.favourites.remove(&item);
        }
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
        let edits = self.favourite_edits;
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
                if feed.favourites_stale || feed.favourite_edits != edits {
                    feed.reload_favourites(cx);
                    return;
                }
                feed.favourites = tracks
                    .into_iter()
                    .map(Playable::Track)
                    .chain(albums.into_iter().map(Playable::Album))
                    .chain(artists.into_iter().map(Playable::Artist))
                    .collect();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
