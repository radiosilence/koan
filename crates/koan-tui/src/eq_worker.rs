//! The EQ page's work, off the frame: reading what a device plays and
//! changing it. Every core call here can block — an overview reads every
//! profile's impulse responses, the output in use asks the audio system — so
//! none of it runs on the 60 fps thread.
//!
//! One thread, one job at a time, in the order they were sent, as the apps'
//! ordered lane does. Jobs that arrive while one runs are taken together:
//! edits to the same band collapse into the last of them, and the page is
//! read again once for the lot, so a held key never queues work behind it.
//! The player is told to reload once per batch that changed anything.

use std::path::PathBuf;

use crossbeam_channel::{Receiver, Sender};
use koan_core::audio::dsp::profiles::{self, Detail, Overview, Response, TargetChoices};
use koan_core::config::DspRole;
use koan_core::player::commands::PlayerCommand;

/// The grid every curve is drawn on, at the rate the apps draw at.
pub const RATE: u32 = 48_000;

/// What the page shows: a device's chain, and one profile open.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ViewReq {
    /// The device picked; `None` is the output in use.
    pub device: Option<String>,
    /// The renderer playing, by UDN and name: the output in use when set.
    pub renderer: Option<(String, String)>,
    pub profile: Option<String>,
}

/// A device's chain and the profiles, as read after the last change.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// The last change sent before this was read.
    pub seq: u64,
    pub in_use: Option<String>,
    pub device: Option<String>,
    pub overview: Overview,
    /// What the device plays, drawn.
    pub response: Option<Response>,
    /// Each stage drawn alone, by profile name.
    pub curves: Vec<(String, Vec<f64>)>,
    /// The outputs to choose among: the one in use first.
    pub devices: Vec<String>,
    /// What to call a device named by its UDN.
    pub names: Vec<(String, String)>,
    /// The chain in words.
    pub sentence: String,
    /// The preset the output in use was set from, and whether it was changed
    /// since; and whether it plays untouched.
    pub in_use_preset: Option<(String, bool)>,
    pub in_use_flat: bool,
    /// The output in use's own EQs, in order, whichever device is shown.
    pub in_use_chain: Vec<(String, bool)>,
    pub profile: Option<ProfileView>,
    /// The profile the page asked for, whether or not it is there.
    pub asked: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProfileView {
    pub detail: Detail,
    pub response: Option<Response>,
    pub targets: Option<TargetChoices>,
    /// What a tuning can be said to be made against: every target, by id
    /// and name.
    pub made_against: Vec<(String, String)>,
}

impl Snapshot {
    pub fn label(&self, device: &str) -> String {
        self.names
            .iter()
            .find(|(id, _)| id == device)
            .map_or_else(|| device.to_owned(), |(_, name)| name.clone())
    }

    pub fn curve(&self, name: &str) -> Option<&[f64]> {
        self.curves
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, db)| db.as_slice())
    }
}

/// What the output in use plays, for Now Playing.
#[derive(Debug, Clone, PartialEq)]
pub struct Playing {
    pub title: String,
    pub freqs: Vec<f64>,
    pub db: Vec<f64>,
}

/// Profiles just imported, whose role is asked once for all of them, and
/// where the import was started.
#[derive(Debug, Clone, PartialEq)]
pub struct RoleAsk {
    pub names: Vec<String>,
    /// What goes into the chain: the group several presets became, or the
    /// profiles made, the first of them as a correction.
    pub profiles: Vec<String>,
    pub into: Option<(String, super::eq_page::Stage)>,
}

/// An import that stopped for the rate of headerless coefficients.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingImport {
    pub paths: Vec<PathBuf>,
    pub name: Option<String>,
    pub into: Option<(String, super::eq_page::Stage)>,
}

/// What a change came to, beyond being done.
#[derive(Debug)]
pub enum Reply {
    Said(String),
    Quiet,
    /// An import finished: ask what its profiles are for, and say `said`.
    Ask(RoleAsk, Option<String>),
    NeedsRate(PendingImport),
    /// Open this profile's page: one just made.
    Open(String),
}

type Run = Box<dyn FnOnce() -> Result<Reply, String> + Send>;

pub struct Act {
    pub seq: u64,
    /// Changes to the same band one after another run once, as the last.
    pub key: Option<(String, usize)>,
    pub run: Run,
}

pub enum Job {
    View(ViewReq),
    Act(Act),
    /// What the output in use plays, for Now Playing: the renderer playing
    /// and the output's rate.
    Playing(Option<String>, u32),
}

pub enum Done {
    Snapshot(Box<Snapshot>),
    Reply {
        seq: u64,
        result: Result<Reply, String>,
    },
    Playing(Option<Playing>),
}

pub struct Worker {
    tx: Sender<Job>,
    pub rx: Receiver<Done>,
}

impl Worker {
    pub fn spawn(player: Sender<PlayerCommand>) -> Self {
        let (tx, jobs) = crossbeam_channel::unbounded();
        let (done, rx) = crossbeam_channel::unbounded();
        std::thread::Builder::new()
            .name("koan-eq".into())
            .spawn(move || run(&jobs, &done, &player))
            .ok();
        Self { tx, rx }
    }

    pub fn send(&self, job: Job) {
        self.tx.send(job).ok();
    }
}

fn run(jobs: &Receiver<Job>, done: &Sender<Done>, player: &Sender<PlayerCommand>) {
    let mut view: Option<ViewReq> = None;
    let mut playing: Option<(Option<String>, u32)> = None;
    let mut seq = 0;
    while let Ok(first) = jobs.recv() {
        let mut acts = Vec::new();
        let (mut read_view, mut read_playing) = (false, false);
        for job in std::iter::once(first).chain(jobs.try_iter()) {
            match job {
                Job::View(v) => {
                    view = Some(v);
                    read_view = true;
                }
                Job::Playing(renderer, rate) => {
                    playing = Some((renderer, rate));
                    read_playing = true;
                }
                Job::Act(a) => acts.push(a),
            }
        }
        let mut changed = false;
        for i in 0..acts.len() {
            seq = seq.max(acts[i].seq);
            // Never across an act without a key: adding or removing a band
            // moves the indices the keys name.
            let superseded = acts[i].key.as_ref().is_some_and(|k| {
                acts[i + 1..]
                    .iter()
                    .map_while(|b| b.key.as_ref())
                    .any(|b| b == k)
            });
            if superseded {
                continue;
            }
            let act = std::mem::replace(
                &mut acts[i],
                Act {
                    seq: 0,
                    key: None,
                    run: Box::new(|| Ok(Reply::Quiet)),
                },
            );
            let result = (act.run)();
            changed |= result.is_ok();
            if done
                .send(Done::Reply {
                    seq: act.seq,
                    result,
                })
                .is_err()
            {
                return;
            }
        }
        if changed {
            player.send(PlayerCommand::ReloadDsp).ok();
        }
        if !acts.is_empty() {
            read_view = true;
            read_playing = true;
        }
        if read_view && let Some(v) = &view {
            let snapshot = load(v, seq);
            if done.send(Done::Snapshot(Box::new(snapshot))).is_err() {
                return;
            }
        }
        if read_playing && let Some((renderer, rate)) = &playing {
            let curve = playing_curve(renderer.clone(), *rate);
            if done.send(Done::Playing(curve)).is_err() {
                return;
            }
        }
    }
}

fn load(v: &ViewReq, seq: u64) -> Snapshot {
    let in_use = v
        .renderer
        .as_ref()
        .map(|(udn, _)| udn.clone())
        .or_else(profiles::current_device);
    let device = v.device.clone().or_else(|| in_use.clone());
    let overview = profiles::overview_for(device.clone());
    let response = device
        .as_deref()
        .and_then(|d| profiles::output_response(d, RATE));
    let curves = overview
        .active
        .iter()
        .chain(overview.chain.iter().map(|(name, _)| name))
        .filter_map(|name| Some((name.clone(), profiles::response(name, RATE)?.total)))
        .collect();
    let mut devices: Vec<String> = Vec::new();
    let mut add = |d: &str| {
        if !devices.iter().any(|x| x == d) {
            devices.push(d.to_owned());
        }
    };
    in_use.iter().chain(device.iter()).for_each(|d| add(d));
    if let Ok(list) = koan_core::audio::list_output_devices() {
        list.iter().for_each(|d| add(&d.name));
    }
    overview
        .profiles
        .iter()
        .filter(|p| !p.preset)
        .flat_map(|p| p.devices.iter())
        .chain(overview.outputs.iter().map(|o| &o.device))
        .for_each(|d| add(d));
    let names = devices
        .iter()
        .filter_map(|d| {
            let name = match &v.renderer {
                Some((udn, name)) if udn == d => name.clone(),
                _ => koan_core::upnp::discovery::find(d)?.name,
            };
            Some((d.clone(), name))
        })
        .collect::<Vec<_>>();
    let sentence = device.as_deref().map_or_else(String::new, |d| {
        let said = profiles::chain_view(d).sentence;
        match names.iter().find(|(id, _)| id == d) {
            Some((_, name)) => said.replace(d, name),
            None => said,
        }
    });
    let (in_use_preset, in_use_flat) = match &in_use {
        Some(d) if Some(d) == device.as_ref() => (
            overview.preset.clone(),
            overview.active.is_none() && overview.chain.is_empty(),
        ),
        Some(d) => (
            profiles::preset_for(d),
            overview
                .outputs
                .iter()
                .find(|o| &o.device == d)
                .is_none_or(|o| o.flat),
        ),
        None => (None, true),
    };
    let profile = v.profile.as_deref().and_then(|name| {
        let detail = profiles::detail(name)?;
        let made_against = if detail.role == DspRole::Tuning {
            use koan_core::audio::dsp::targets::{self, Ear};
            targets::TARGETS
                .iter()
                .filter(|t| t.ear != Ear::Speaker)
                .map(|t| (t.id.to_owned(), t.name.to_owned()))
                .chain(targets::added().into_iter().map(|a| (a.id, a.name)))
                .collect()
        } else {
            Vec::new()
        };
        Some(ProfileView {
            response: profiles::response(name, RATE),
            targets: profiles::target_choices(name),
            made_against,
            detail,
        })
    });
    let in_use_chain = in_use
        .as_deref()
        .map(profiles::tunings_for)
        .unwrap_or_default();
    Snapshot {
        seq,
        in_use,
        device,
        overview,
        response,
        curves,
        devices,
        names,
        sentence,
        in_use_preset,
        in_use_flat,
        in_use_chain,
        profile,
        asked: v.profile.clone(),
    }
}

/// What the output in use plays at `rate`, and what to call it: its preset,
/// or the correction and the EQs on it.
fn playing_curve(renderer: Option<String>, rate: u32) -> Option<Playing> {
    let device = renderer.or_else(profiles::current_device)?;
    let r = profiles::output_response(&device, rate)?;
    let chain = profiles::chain_view(&device);
    let title = match (&chain.preset, chain.edited) {
        (Some(p), false) => p.clone(),
        (Some(p), true) => format!("{p}, edited"),
        (None, _) => chain
            .correction
            .iter()
            .cloned()
            .chain(
                chain
                    .tuning
                    .iter()
                    .filter(|t| t.on && !chain.left_out.contains(&t.name))
                    .map(|t| t.name.clone()),
            )
            .collect::<Vec<_>>()
            .join(" + "),
    };
    Some(Playing {
        title,
        freqs: r.freqs,
        db: r.total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// Edits to one band sent while the worker is busy run once, as the
    /// last of them, and the player is told to reload once.
    #[test]
    fn a_burst_of_edits_to_one_band_runs_as_its_last() {
        let (player, commands) = crossbeam_channel::unbounded();
        let worker = Worker::spawn(player);
        let ran = Arc::new(Mutex::new(Vec::new()));
        let gate = Arc::new(Mutex::new(()));
        let held = gate.lock().unwrap();
        // The first job holds the worker until the rest are queued.
        let g = gate.clone();
        worker.send(Job::Act(Act {
            seq: 1,
            key: None,
            run: Box::new(move || {
                drop(g.lock().unwrap());
                Ok(Reply::Quiet)
            }),
        }));
        // Give the worker the first job before the rest are sent.
        std::thread::sleep(std::time::Duration::from_millis(50));
        for seq in 2..=6 {
            let ran = ran.clone();
            worker.send(Job::Act(Act {
                seq,
                key: Some(("Warm".into(), 0)),
                run: Box::new(move || {
                    ran.lock().unwrap().push(seq);
                    Ok(Reply::Quiet)
                }),
            }));
        }
        let other = Arc::new(AtomicUsize::new(0));
        let o = other.clone();
        worker.send(Job::Act(Act {
            seq: 7,
            key: Some(("Warm".into(), 1)),
            run: Box::new(move || {
                o.fetch_add(1, Ordering::SeqCst);
                Ok(Reply::Quiet)
            }),
        }));
        drop(held);
        let mut replies = Vec::new();
        while replies.len() < 3 {
            if let Done::Reply { seq, .. } = worker
                .rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
            {
                replies.push(seq);
            }
        }
        // The first ran alone; the rest arrived together and collapsed.
        let ran = ran.lock().unwrap().clone();
        assert_eq!(ran, vec![6]);
        assert_eq!(other.load(Ordering::SeqCst), 1);
        assert_eq!(replies, vec![1, 6, 7]);
        let reloads = std::iter::from_fn(|| {
            commands
                .recv_timeout(std::time::Duration::from_millis(200))
                .ok()
        })
        .filter(|c| matches!(c, PlayerCommand::ReloadDsp))
        .count();
        assert_eq!(reloads, 2, "once per batch that changed anything");
    }

    /// An edit to a band, a band removed, then the same band edited: both
    /// edits run, since the removal moved what the index names.
    #[test]
    fn edits_are_never_collapsed_across_a_removal() {
        let (player, _commands) = crossbeam_channel::unbounded();
        let worker = Worker::spawn(player);
        let ran = Arc::new(Mutex::new(Vec::new()));
        let gate = Arc::new(Mutex::new(()));
        let held = gate.lock().unwrap();
        let g = gate.clone();
        worker.send(Job::Act(Act {
            seq: 1,
            key: None,
            run: Box::new(move || {
                drop(g.lock().unwrap());
                Ok(Reply::Quiet)
            }),
        }));
        std::thread::sleep(std::time::Duration::from_millis(50));
        for (seq, key) in [
            (2, Some(("Warm".to_owned(), 3))),
            (3, None),
            (4, Some(("Warm".to_owned(), 3))),
        ] {
            let ran = ran.clone();
            worker.send(Job::Act(Act {
                seq,
                key,
                run: Box::new(move || {
                    ran.lock().unwrap().push(seq);
                    Ok(Reply::Quiet)
                }),
            }));
        }
        drop(held);
        let mut replies = 0;
        while replies < 4 {
            if let Done::Reply { .. } = worker
                .rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
            {
                replies += 1;
            }
        }
        assert_eq!(*ran.lock().unwrap(), vec![2, 3, 4]);
    }
}
