//! Which kōan the terminal shows and commands: this one, or another device
//! it controls, as the apps' Play on does.
//!
//! Every command the terminal sends goes through `router`, which hands it to
//! this player or, while another device is controlled, to that device
//! (`control::send`). What the pages read is `View::state`: this player's
//! state, or a mirror of the controlled device's last report.

use std::path::PathBuf;
use std::sync::Arc;

use crossbeam_channel::Sender;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{
    ItemState, PlayMode, PlaybackState, PlaylistItem, QueueItemId, SharedPlayerState, TrackInfo,
};
use koan_core::remote::devices::{self, Device, Reach};
use koan_core::remote::link::{LinkQueueEntry, LinkState};

/// A channel that sends each command where the terminal is pointed, and the
/// lane that changes where that is. What does not get there is said on the
/// status line, through `report`.
///
/// Unbounded, since sending to a device can wait on the network and the
/// frame must not wait behind it.
pub fn router(
    local: Sender<PlayerCommand>,
    report: Sender<crate::tasks::Report>,
) -> (Sender<PlayerCommand>, Lane) {
    let (tx, rx) = crossbeam_channel::unbounded::<PlayerCommand>();
    let (work, works) = crossbeam_channel::unbounded::<(Sender<()>, Work)>();
    std::thread::Builder::new()
        .name("koan-control".into())
        .spawn(move || {
            let mut waiting: Vec<(Sender<()>, Work)> = Vec::new();
            for cmd in rx {
                if let PlayerCommand::Barrier(mark) = &cmd {
                    waiting.extend(works.try_iter());
                    if let Some(at) = waiting.iter().position(|(m, _)| m.same_channel(mark)) {
                        (waiting.remove(at).1)();
                        continue;
                    }
                }
                if let Err(e) = koan_core::remote::control::send(cmd, &local) {
                    log::warn!("play on: {e}");
                    report.send(crate::tasks::Report::Work(e.to_string())).ok();
                }
            }
        })
        .expect("failed to spawn the control thread");
    (tx.clone(), Lane { tx, work })
}

type Work = Box<dyn FnOnce() + Send>;

/// Work done in order with the commands around it: choosing a device, or
/// moving the music, which change where every command after them goes.
pub struct Lane {
    tx: Sender<PlayerCommand>,
    work: Sender<(Sender<()>, Work)>,
}

impl Lane {
    /// Run `work` on the router once every command sent before it has gone,
    /// and before any sent after it. Its place among them is held by a
    /// barrier whose channel names it; the router runs the work there rather
    /// than passing the barrier on.
    pub fn run(&self, work: impl FnOnce() + Send + 'static) {
        let (mark, _) = crossbeam_channel::bounded(0);
        if self.work.send((mark.clone(), Box::new(work))).is_ok() {
            self.tx.send(PlayerCommand::Barrier(mark)).ok();
        }
    }
}

/// The state the pages draw, kept up with the device being controlled.
pub struct View {
    pub local: Arc<SharedPlayerState>,
    /// The device being controlled, by name, and how it can be reached.
    pub controlled: Option<(String, Reach)>,
    mirror: Arc<SharedPlayerState>,
    /// The devices' version last mirrored, and the target then.
    seen: Option<(u64, Option<String>)>,
    queue: Vec<LinkQueueEntry>,
}

impl View {
    pub fn new(local: Arc<SharedPlayerState>) -> Self {
        Self {
            local,
            controlled: None,
            mirror: SharedPlayerState::new(),
            seen: None,
            queue: Vec::new(),
        }
    }

    /// The state to draw this frame. The list of devices is read only when
    /// it has moved; the playhead is worked out from the last report.
    pub fn state(&mut self) -> &Arc<SharedPlayerState> {
        let target = devices::target();
        if target.is_none() {
            self.seen = None;
            self.controlled = None;
            return &self.local;
        }
        let version = devices::version();
        if self.seen.as_ref() != Some(&(version, target.clone())) {
            let device = devices::target_device();
            let name = device.as_ref().map_or_else(
                || devices::name_of(target.as_deref().unwrap_or_default()),
                |d| Some(d.name.clone()),
            );
            self.controlled = Some((
                name.unwrap_or_else(|| "the device".into()),
                device
                    .as_ref()
                    .map_or(Reach::OutOfReach(None), Device::reach),
            ));
            self.seen = Some((version, target));
            self.mirror(device.as_ref());
        }
        if let Some((position, _)) = devices::target_playhead() {
            self.mirror.set_position_ms(position);
        }
        &self.mirror
    }

    fn mirror(&mut self, device: Option<&Device>) {
        let m = &self.mirror;
        let Some((device, report)) = device.and_then(|d| Some((d, d.state.as_ref()?))) else {
            if !self.queue.is_empty() || m.track_info().is_some() {
                self.queue.clear();
                m.restore_playlist(Vec::new(), None);
                m.set_track_info(None);
            }
            m.set_playback_state(PlaybackState::Stopped);
            m.set_play_mode(PlayMode::default());
            m.set_sleep(None);
            m.set_position_ms(0);
            return;
        };
        if report.queue != self.queue {
            self.queue = report.queue.clone();
            let local = local_ids(device, &self.queue);
            let items: Vec<PlaylistItem> = self
                .queue
                .iter()
                .zip(local)
                .enumerate()
                .map(|(n, (e, db_id))| PlaylistItem {
                    db_id,
                    ..item(n, e)
                })
                .collect();
            let cursor = self
                .queue
                .iter()
                .position(|e| e.current)
                .map(|n| items[n].id);
            m.restore_playlist(items, cursor);
        }
        m.set_track_info(m.cursor().map(|id| TrackInfo {
            id,
            path: PathBuf::from(format!("/remote/{}", id.0)),
            codec: format!("on {}", device.name),
            sample_rate: 0,
            bit_depth: None,
            bitrate_kbps: None,
            channels: 2,
            duration_ms: report.duration_ms,
        }));
        m.set_playback_state(state_of(report));
        m.set_play_mode(PlayMode {
            shuffle: report.shuffle,
            repeat: report.repeat,
        });
        m.set_sleep(report.sleep);
        m.set_position_ms(device.position_ms());
    }
}

/// The tracks of a device playing from this library, as this library's own:
/// what hearts, covers and track info name them by. Never a sync: a track
/// this library lacks stays unnamed.
fn local_ids(device: &Device, queue: &[LinkQueueEntry]) -> Vec<Option<i64>> {
    let none = || vec![None; queue.len()];
    if !device.same_library {
        return none();
    }
    let Ok(db) = koan_core::db::pool::shared().get() else {
        return none();
    };
    let named: Vec<(usize, String)> = queue
        .iter()
        .enumerate()
        .filter_map(|(n, e)| e.track_id.clone().map(|t| (n, t)))
        .collect();
    let ids: Vec<String> = named.iter().map(|(_, t)| t.clone()).collect();
    let (found, _) = koan_core::remote::link::resolve_each(&db, &ids, false);
    let mut out = none();
    for ((n, _), id) in named.into_iter().zip(found) {
        out[n] = id;
    }
    out
}

fn state_of(report: &LinkState) -> PlaybackState {
    if report.playing {
        PlaybackState::Playing
    } else if report.title.is_some() {
        PlaybackState::Paused
    } else {
        PlaybackState::Stopped
    }
}

/// A queue entry of the device being controlled, under the id that device
/// gave it, which is what a command naming it carries back.
fn item(n: usize, e: &LinkQueueEntry) -> PlaylistItem {
    let id =
        e.id.as_deref()
            .and_then(|id| uuid::Uuid::parse_str(id).ok())
            .map_or(QueueItemId(uuid::Uuid::from_u128(n as u128)), QueueItemId);
    PlaylistItem {
        id,
        db_id: None,
        uid: None,
        playlist_entry_id: None,
        path: PathBuf::from(format!("/remote/{}", id.0)),
        title: e.title.clone(),
        artist: e.artist.clone(),
        album_artist: e.artist.clone(),
        album: e.album.clone(),
        year: None,
        codec: None,
        track_number: None,
        disc: None,
        duration_ms: (e.duration_ms > 0).then_some(e.duration_ms),
        state: ItemState::Ready,
        played: false,
    }
}

/// A row of the Play on chooser: this device, then each other one.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    /// `None` for this device.
    pub id: Option<String>,
    pub name: String,
    pub detail: String,
    pub controlled: bool,
    /// Can be chosen: reachable, or able to be woken.
    pub reachable: bool,
    /// Asleep, so it can be forgotten.
    pub away: bool,
    /// The music being controlled can be moved there.
    pub can_move: bool,
}

pub struct PlayOnState {
    pub choices: Vec<Choice>,
    pub cursor: usize,
    /// The devices' version the choices were read at.
    seen: u64,
}

impl PlayOnState {
    pub fn new(local: &SharedPlayerState) -> Self {
        let mut s = Self {
            choices: Vec::new(),
            cursor: 0,
            seen: u64::MAX,
        };
        s.refresh(local);
        if let Some(at) = s.choices.iter().position(|c| c.controlled) {
            s.cursor = at;
        }
        s
    }

    /// Read the devices again if they have moved since.
    pub fn refresh(&mut self, local: &SharedPlayerState) {
        let version = devices::version();
        if version == self.seen {
            return;
        }
        self.seen = version;
        self.choices = choices(local);
        self.cursor = self.cursor.min(self.choices.len().saturating_sub(1));
    }

    pub fn move_by(&mut self, delta: isize) {
        if !self.choices.is_empty() {
            self.cursor =
                (self.cursor as isize + delta).rem_euclid(self.choices.len() as isize) as usize;
        }
    }

    pub fn current(&self) -> Option<&Choice> {
        self.choices.get(self.cursor)
    }
}

fn choices(local: &SharedPlayerState) -> Vec<Choice> {
    let target = devices::target();
    let listed = devices::list();
    let controlled = target
        .as_ref()
        .and_then(|t| listed.iter().find(|d| d.id == *t));
    // Moving takes music: the device in view has to have some, from a
    // library the destination plays from too.
    let has_music = match (&target, controlled) {
        (None, _) => !local.is_empty(),
        (Some(_), Some(d)) => {
            d.same_library
                && d.problem.is_none()
                && d.state.as_ref().is_some_and(|s| s.title.is_some())
        }
        (Some(_), None) => false,
    };
    let mut out = vec![Choice {
        id: None,
        name: "this terminal".into(),
        detail: if target.is_some() {
            "paused while you control another device".into()
        } else {
            String::new()
        },
        controlled: target.is_none(),
        reachable: true,
        away: false,
        can_move: target.is_some() && has_music,
    }];
    out.extend(listed.iter().map(|d| Choice {
        id: Some(d.id.clone()),
        name: d.name.clone(),
        detail: detail(d),
        controlled: target.as_deref() == Some(d.id.as_str()),
        reachable: d.problem.is_none() && (d.awake || d.wakeable),
        away: !d.awake,
        can_move: target.as_deref() != Some(d.id.as_str())
            && d.problem.is_none()
            && d.same_library
            && has_music,
    }));
    out
}

/// What a device is doing, or why it cannot be reached.
fn detail(d: &Device) -> String {
    let owner = d.owner.as_ref().map(|o| format!("shared by {o} · "));
    let what = if let Some(problem) = &d.problem {
        problem.clone()
    } else if d.waking.is_some() {
        "waking...".into()
    } else if !d.awake && !d.asleep {
        "reconnecting...".into()
    } else if !d.awake {
        let seen = d
            .last_seen
            .map(|at| format!(" · seen {}", ago(at)))
            .unwrap_or_default();
        if d.wakeable {
            format!("asleep{seen}; choosing it wakes it")
        } else {
            format!("asleep{seen}")
        }
    } else {
        let library = if d.same_library {
            ""
        } else {
            " · different library"
        };
        let way = if d.nearby { "" } else { " · via server" };
        match &d.state {
            Some(s) if s.title.is_some() => {
                let playing = [s.title.as_deref(), s.artist.as_deref()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" \u{2014} ");
                let how = if s.playing { "playing" } else { "paused" };
                format!("{how} {playing}{library}{way}")
            }
            _ => format!("idle{library}{way}"),
        }
    };
    owner.unwrap_or_default() + &what
}

/// "5m ago", from Unix seconds.
fn ago(at: i64) -> String {
    let secs = (chrono::Utc::now().timestamp() - at).max(0);
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{}m ago", secs / 60),
        3600..86400 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_queue_entry_keeps_the_id_its_device_gave_it() {
        let id = uuid::Uuid::now_v7();
        let entry = LinkQueueEntry {
            id: Some(id.to_string()),
            title: "t".into(),
            duration_ms: 1000,
            ..Default::default()
        };
        assert_eq!(item(3, &entry).id, QueueItemId(id));
        let anonymous = LinkQueueEntry::default();
        assert_eq!(item(3, &anonymous).id, item(3, &anonymous).id);
        assert_ne!(item(3, &anonymous).id, item(4, &anonymous).id);
    }

    #[test]
    fn a_device_that_reports_nothing_playing_is_stopped() {
        let mut s = LinkState::default();
        assert_eq!(state_of(&s), PlaybackState::Stopped);
        s.title = Some("x".into());
        assert_eq!(state_of(&s), PlaybackState::Paused);
        s.playing = true;
        assert_eq!(state_of(&s), PlaybackState::Playing);
    }

    #[test]
    fn times_ago_read_in_the_largest_unit() {
        let now = chrono::Utc::now().timestamp();
        assert_eq!(ago(now), "just now");
        assert_eq!(ago(now - 300), "5m ago");
        assert_eq!(ago(now - 7200), "2h ago");
    }
}
