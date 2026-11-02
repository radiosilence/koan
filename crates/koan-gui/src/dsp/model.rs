//! EQ and convolution profiles, and importing them: what every EQ view reads
//! and every EQ write goes through.
//!
//! One entity for the app. A write is an engine call awaited on the main
//! thread's executor (koan-ffi's futures need no runtime and run their work
//! on its own lanes), after which the overview is read again and `version`
//! moves, which is what a page showing a profile reloads on. Profiles synced
//! from another device arrive as a library change, which moves `library`.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use koan_ffi::{
    AutoEqEntry, AutoEqMaker, AutoEqOffer, ConnectionInfo, DspBand, DspBandBounds, DspEdited,
    DspImportPlan, DspImportSummary, DspOverview, DspPoint, DspProfileSummary, DspRole,
    DspTuningEntry, KoanEngine, KoanError,
};
use koan_kit::app::{App, AppContext, Context, Entity, SharedString, Subscription, Task};

use crate::mirror::{Mirror, Slice};

/// What a server lists when it keeps the account's EQs on every device.
pub const SYNC_EXTENSION: &str = "koanDspProfiles";
/// What a server lists when it keeps deleted EQs for thirty days.
pub const DELETED_EXTENSION: &str = "koanDspDeleted";

/// Rates coefficients are commonly designed at.
pub const RATES: [u32; 6] = [44100, 48000, 88200, 96000, 176_400, 192_000];

/// The rate the graphs are drawn at.
pub const GRAPH_RATE: u32 = 48000;

/// A stage of a device's chain, to choose what fills it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Correction,
    Eq,
}

/// The stage of a device's chain an import was started from, which what it
/// made goes into once its role is answered.
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub device: String,
    pub stage: Stage,
}

/// Files chosen together, and what importing them will do.
#[derive(Clone, Debug)]
pub struct PendingImport {
    pub paths: Vec<PathBuf>,
    pub plan: DspImportPlan,
    pub rate: Option<u32>,
    pub into: Option<Placement>,
}

/// The profiles an import is asking about, and where it was started.
#[derive(Clone, Debug, PartialEq)]
pub struct RoleAsk {
    pub names: Vec<String>,
    /// What goes into the chain: the group several presets became, or the
    /// profiles made, the first of them as a correction.
    pub profiles: Vec<String>,
    pub into: Option<Placement>,
}

/// An import waiting on the rate of what it was given.
#[derive(Clone, Debug)]
pub enum NeedsRate {
    Files {
        paths: Vec<PathBuf>,
        name: Option<String>,
        into: Option<Placement>,
    },
    Text(String),
}

pub struct Dsp {
    engine: Arc<KoanEngine>,
    pub overview: Option<DspOverview>,
    /// AutoEQ's profile for the output in use, while it has none and the
    /// offer has not been turned down.
    pub suggestion: Option<AutoEqOffer>,
    pub last_error: Option<SharedString>,
    /// What the last import of several files did.
    pub import_summary: Option<SharedString>,
    /// Profiles just imported, whose role is asked once for all of them.
    pub ask_role: Option<RoleAsk>,
    /// Several files planned for import, waiting to be confirmed and named.
    pub pending_import: Option<PendingImport>,
    pub needs_rate: Option<NeedsRate>,
    pub autoeq_results: Vec<AutoEqEntry>,
    pub autoeq_makers: Vec<AutoEqMaker>,
    /// Moves with every AutoEQ search, so one that returns after a newer one
    /// began is dropped.
    autoeq_search: u64,
    _autoeq: Option<Task<()>>,
    /// The ranges a band is held to and the types it can be, from the core.
    pub bounds: DspBandBounds,
    /// Moves on every change made here.
    version: u64,
    /// Overview reads started, and the newest one applied.
    reads: u64,
    applied: u64,
    /// Edits made here, by profile.
    edits: HashMap<String, u64>,
    /// The saves of each edit under way, by its number.
    lanes: HashMap<u64, Lane>,
    /// Edits refused or failed, whose later steps go nowhere.
    closed: HashSet<u64>,
    /// The filter each finished edit last stored, for its next step.
    stored: HashMap<u64, DspBand>,
    /// Edits forgotten while a save of theirs was in flight.
    forgotten: HashSet<u64>,
    edit_count: u64,
    /// The library's version, which moves when profiles sync from another
    /// device.
    library: u64,
    /// The output this device plays through and the outputs it has, as the
    /// mirror last said: the overview is of the device that resolves to, and
    /// with the system's default chosen, plugging in a DAC moves it.
    output: Option<(koan_ffi::OutputChoice, Vec<String>)>,
    /// The extensions the signed-in server lists, and whether it is out
    /// of reach.
    extensions: Vec<String>,
    pub offline: bool,
    _observe: Vec<Subscription>,
}

impl Dsp {
    pub fn new(engine: Arc<KoanEngine>, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            let observe = Self::observe(cx);
            let mut dsp = Self {
                engine,
                overview: None,
                suggestion: None,
                last_error: None,
                import_summary: None,
                ask_role: None,
                pending_import: None,
                needs_rate: None,
                autoeq_results: Vec::new(),
                autoeq_makers: Vec::new(),
                autoeq_search: 0,
                _autoeq: None,
                bounds: koan_ffi::dsp_band_bounds(),
                version: 0,
                reads: 0,
                applied: 0,
                edits: HashMap::new(),
                lanes: HashMap::new(),
                closed: HashSet::new(),
                stored: HashMap::new(),
                forgotten: HashSet::new(),
                edit_count: 0,
                library: 0,
                output: None,
                extensions: Vec::new(),
                offline: false,
                _observe: observe,
            };
            let connection = cx.global::<Mirror>().connection.clone();
            dsp.connection(&connection, cx);
            dsp.library = cx.global::<Mirror>().library.read(cx).0;
            dsp.reload(cx);
            dsp
        })
    }

    /// Follows the mirror's library, output and connection: profiles synced
    /// from another device arrive as a library change, the overview is of
    /// the output in use, and what the server offers decides Sync and
    /// Recently deleted.
    fn observe(cx: &mut Context<Self>) -> Vec<Subscription> {
        let mirror = cx.global::<Mirror>();
        let (library, connection, outputs) = (
            mirror.library.clone(),
            mirror.connection.clone(),
            mirror.outputs.clone(),
        );
        vec![
            cx.observe(&outputs, |dsp: &mut Dsp, outputs, cx| {
                // Another device's outputs are that device's to equalise.
                let output = outputs
                    .read(cx)
                    .0
                    .as_ref()
                    .filter(|o| o.owner.is_none())
                    .map(|o| {
                        let ids = o.devices.iter().chain(&o.renderers).map(|d| d.id.clone());
                        (o.current.clone(), ids.collect())
                    });
                if output.is_some() && output != dsp.output {
                    let first = dsp.output.is_none();
                    dsp.output = output;
                    if !first {
                        dsp.changed(cx);
                    }
                }
            }),
            cx.observe(&library, |dsp: &mut Dsp, library, cx| {
                let version = library.read(cx).0;
                if version != dsp.library {
                    dsp.library = version;
                    dsp.reload(cx);
                }
            }),
            cx.observe(&connection, |dsp: &mut Dsp, connection, cx| {
                dsp.connection(&connection, cx)
            }),
        ]
    }

    fn connection(
        &mut self,
        connection: &Entity<Slice<Option<ConnectionInfo>>>,
        cx: &mut Context<Self>,
    ) {
        let (extensions, offline) = match &connection.read(cx).0 {
            Some(c) => (
                c.extensions.iter().map(|e| e.name.clone()).collect(),
                c.offline,
            ),
            None => (Vec::new(), false),
        };
        if extensions != self.extensions || offline != self.offline {
            self.extensions = extensions;
            self.offline = offline;
            cx.notify();
        }
    }

    pub fn engine(&self) -> Arc<KoanEngine> {
        self.engine.clone()
    }

    /// Moves on every change made here or synced from elsewhere: what a page
    /// showing profiles reloads on.
    pub fn stamp(&self) -> (u64, u64) {
        (self.version, self.library)
    }

    /// Whether the signed-in server lists `extension`.
    pub fn offers(&self, extension: &str) -> bool {
        self.extensions.iter().any(|e| e == extension)
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.read(false, cx, |_, _| {});
    }

    /// Reads the overview, moving `version` once it lands if `bump`, and
    /// tells `then` the stamp. Reads run in parallel, so one that lands
    /// after a newer one has is dropped: a read started before a write
    /// never puts the overview from before it back.
    fn read(
        &mut self,
        bump: bool,
        cx: &mut Context<Self>,
        then: impl FnOnce((u64, u64), &mut App) + 'static,
    ) {
        self.reads += 1;
        let read = self.reads;
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            // What AutoEQ offers follows what the output plays, so it is read
            // with it.
            let (overview, suggestion) = futures::join!(
                engine.clone().dsp_overview(),
                engine.clone().autoeq_suggestion()
            );
            let Ok(stamp) = this.update(cx, |dsp, cx| {
                if read > dsp.applied {
                    dsp.applied = read;
                    dsp.overview = Some(overview);
                    dsp.suggestion = suggestion;
                }
                if bump {
                    dsp.version += 1;
                }
                cx.notify();
                dsp.stamp()
            }) else {
                return;
            };
            cx.update(|cx| then(stamp, cx));
        })
        .detach();
    }

    /// The overview read again after a change, and `version` moved.
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.read(true, cx, |_, _| {});
    }

    /// Runs a write, says why it failed if it did, and reads the overview
    /// again either way.
    pub fn act<F, Fut>(&mut self, cx: &mut Context<Self>, run: F)
    where
        F: FnOnce(Arc<KoanEngine>) -> Fut,
        Fut: Future<Output = Result<(), KoanError>> + 'static,
    {
        self.act_then(cx, run, |_, _| {});
    }

    /// As `act`, telling `done` whether it took once the overview is read
    /// again.
    pub fn act_then<F, Fut, T>(
        &mut self,
        cx: &mut Context<Self>,
        run: F,
        done: impl FnOnce(Option<T>, &mut App) + 'static,
    ) where
        F: FnOnce(Arc<KoanEngine>) -> Fut,
        Fut: Future<Output = Result<T, KoanError>> + 'static,
        T: 'static,
    {
        let job = run(self.engine.clone());
        cx.spawn(async move |this, cx| {
            let result = job.await;
            this.update(cx, |dsp, cx| {
                let (taken, error) = match result {
                    Ok(t) => (Some(t), None),
                    Err(e) => (None, Some(describe(&e))),
                };
                dsp.last_error = error;
                dsp.read(true, cx, move |_, cx| done(taken, cx));
            })
            .ok();
        })
        .detach();
    }

    // MARK: - Importing

    /// Files, folders or zips. Several are first planned and shown to be
    /// confirmed (`pending_import`): whole presets become a group, parts of
    /// one profile combine into one. Started from a stage of a device's
    /// chain, what it makes goes into it.
    pub fn import_files(
        &mut self,
        paths: Vec<PathBuf>,
        name: Option<String>,
        rate: Option<u32>,
        into: Option<Placement>,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        if paths.len() > 1 && name.is_none() {
            let engine = self.engine.clone();
            let shown = paths.iter().map(|p| p.display().to_string()).collect();
            cx.spawn(async move |this, cx| {
                let plan = engine.dsp_import_plan(shown).await;
                this.update(cx, |dsp, cx| {
                    dsp.pending_import = Some(PendingImport {
                        paths,
                        plan,
                        rate,
                        into,
                    });
                    cx.notify();
                })
                .ok();
            })
            .detach();
            return;
        }
        self.run_import(paths, name, rate, into, cx);
    }

    /// Imports what `pending_import` planned, under `name`.
    pub fn confirm_import(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_import.take() else {
            return;
        };
        cx.notify();
        self.run_import(pending.paths, Some(name), pending.rate, pending.into, cx);
    }

    pub fn cancel_import(&mut self, cx: &mut Context<Self>) {
        if self.pending_import.take().is_some() {
            cx.notify();
        }
    }

    fn run_import(
        &mut self,
        paths: Vec<PathBuf>,
        name: Option<String>,
        rate: Option<u32>,
        into: Option<Placement>,
        cx: &mut Context<Self>,
    ) {
        let engine = self.engine.clone();
        let shown: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
        let files = paths.len();
        cx.spawn(async move |this, cx| {
            let result = engine.dsp_import_files(shown, name.clone(), rate).await;
            this.update(cx, |dsp, cx| {
                match result {
                    Ok(summary) => {
                        dsp.last_error = None;
                        if !summary.imported.is_empty() {
                            dsp.ask_role = Some(RoleAsk {
                                profiles: summary
                                    .group
                                    .clone()
                                    .map(|g| vec![g])
                                    .unwrap_or_else(|| summary.imported.clone()),
                                names: summary.imported.clone(),
                                into,
                            });
                        }
                        dsp.import_summary = describe_import(&summary, files).map(Into::into);
                    }
                    Err(KoanError::NeedsSampleRate { .. }) => {
                        dsp.needs_rate = Some(NeedsRate::Files { paths, name, into });
                    }
                    Err(e) => {
                        dsp.import_summary = None;
                        dsp.last_error = Some(describe(&e));
                    }
                }
                dsp.changed(cx);
            })
            .ok();
        })
        .detach();
    }

    pub fn import_text(&mut self, text: String, rate: Option<u32>, cx: &mut Context<Self>) {
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let result = engine.dsp_import_text(text.clone(), None, rate).await;
            this.update(cx, |dsp, cx| {
                match result {
                    Ok(name) => {
                        dsp.last_error = None;
                        dsp.ask_role = Some(RoleAsk {
                            names: vec![name.clone()],
                            profiles: vec![name],
                            into: None,
                        });
                    }
                    Err(KoanError::NeedsSampleRate { .. }) => {
                        dsp.needs_rate = Some(NeedsRate::Text(text));
                    }
                    Err(e) => dsp.last_error = Some(describe(&e)),
                }
                dsp.changed(cx);
            })
            .ok();
        })
        .detach();
    }

    /// The import that stopped for a rate, with one.
    pub fn retry(&mut self, rate: u32, cx: &mut Context<Self>) {
        match self.needs_rate.take() {
            Some(NeedsRate::Files { paths, name, into }) => {
                self.import_files(paths, name, Some(rate), into, cx)
            }
            Some(NeedsRate::Text(text)) => self.import_text(text, Some(rate), cx),
            None => {}
        }
        cx.notify();
    }

    pub fn cancel_rate(&mut self, cx: &mut Context<Self>) {
        if self.needs_rate.take().is_some() {
            cx.notify();
        }
    }

    /// Answers an import's question: what each of its profiles is for, none
    /// leaving them tunings. One started from a device's chain then goes
    /// into it: a correction as the device's correction, a tuning on the end
    /// of its filters.
    pub fn answer(&mut self, ask: RoleAsk, role: Option<DspRole>, cx: &mut Context<Self>) {
        if self.ask_role.as_ref() == Some(&ask) {
            self.ask_role = None;
        }
        self.act(cx, move |engine| async move {
            if let Some(role) = role {
                for name in &ask.names {
                    engine.clone().dsp_set_role(name.clone(), role).await?;
                }
            }
            let Some(into) = ask.into else {
                return Ok(());
            };
            if matches!(role, Some(DspRole::Correction | DspRole::Baked)) {
                engine
                    .dsp_assign_device(into.device, ask.profiles.first().cloned())
                    .await?;
            } else if into.stage == Stage::Eq {
                let chain = engine
                    .clone()
                    .dsp_overview_for(Some(into.device.clone()))
                    .await
                    .chain;
                let added: Vec<_> = ask
                    .profiles
                    .iter()
                    .filter(|name| !chain.iter().any(|e| &e.name == *name))
                    .map(|name| DspTuningEntry {
                        name: name.clone(),
                        on: true,
                    })
                    .collect();
                if !added.is_empty() {
                    let mut tuning = chain;
                    tuning.extend(added);
                    engine.dsp_set_tunings(into.device, tuning).await?;
                }
            }
            Ok(())
        });
    }

    // MARK: - AutoEQ

    /// Searches AutoEQ by headphone name once typing settles; a search
    /// overtaken by a newer one is dropped.
    pub fn search_autoeq(&mut self, query: String, cx: &mut Context<Self>) {
        self.autoeq_search += 1;
        let search = self.autoeq_search;
        let trimmed = query.trim().to_owned();
        if trimmed.is_empty() {
            self._autoeq = None;
            self.autoeq_results.clear();
            cx.notify();
            return;
        }
        let engine = self.engine.clone();
        self._autoeq = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(250))
                .await;
            let found = engine.autoeq_search(trimmed, 40).await;
            this.update(cx, |dsp, cx| {
                if search != dsp.autoeq_search {
                    return;
                }
                match found {
                    Ok(found) => dsp.autoeq_results = found,
                    Err(e) => dsp.last_error = Some(describe(&e)),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// The makers to browse, read once: the index changes daily at most.
    pub fn load_autoeq_makers(&mut self, cx: &mut Context<Self>) {
        if !self.autoeq_makers.is_empty() {
            return;
        }
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let makers = engine.autoeq_makers().await;
            this.update(cx, |dsp, cx| {
                match makers {
                    Ok(makers) => dsp.autoeq_makers = makers,
                    Err(e) => dsp.last_error = Some(describe(&e)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Installs `entry` as a profile and plays the output in use through it.
    pub fn install_autoeq(&mut self, entry: AutoEqEntry, cx: &mut Context<Self>) {
        let device = self.overview.as_ref().and_then(|o| o.device.clone());
        self.suggestion = None;
        self.act(cx, move |engine| async move {
            engine
                .autoeq_install(entry.name, entry.measured_by, device)
                .await
                .map(|_| ())
        });
    }

    /// Stops offering AutoEQ's profile for the output in use.
    pub fn dismiss_suggestion(&mut self, cx: &mut Context<Self>) {
        self.suggestion = None;
        self.act(cx, |engine| engine.autoeq_dismiss());
    }

    // MARK: - Choosing

    /// What to call a device: a renderer by its name rather than its UDN.
    pub fn label(&self, device: &str) -> String {
        self.overview
            .as_ref()
            .and_then(|o| o.names.get(device).cloned())
            .unwrap_or_else(|| device.to_owned())
    }

    pub fn profile(&self, name: &str) -> Option<&DspProfileSummary> {
        self.overview
            .as_ref()
            .and_then(|o| o.profiles.iter().find(|p| p.name == name))
    }

    /// Plays `device` through `profile`, or untouched.
    pub fn assign(&mut self, profile: Option<String>, device: String, cx: &mut Context<Self>) {
        self.act(cx, move |engine| engine.dsp_assign_device(device, profile));
    }

    /// Makes `device`'s filters these EQs, in the order they play.
    pub fn set_tunings(
        &mut self,
        tuning: Vec<DspTuningEntry>,
        device: String,
        cx: &mut Context<Self>,
    ) {
        self.act(cx, move |engine| engine.dsp_set_tunings(device, tuning));
    }

    /// Makes `device`'s filters these EQs, then stops correcting it: should
    /// the second fail, the device keeps its correction.
    pub fn uncorrect(
        &mut self,
        device: String,
        tuning: Vec<DspTuningEntry>,
        cx: &mut Context<Self>,
    ) {
        self.act(cx, move |engine| async move {
            engine
                .clone()
                .dsp_set_tunings(device.clone(), tuning)
                .await?;
            engine.dsp_assign_device(device, None).await
        });
    }

    pub fn apply_preset(&mut self, name: Option<String>, device: String, cx: &mut Context<Self>) {
        self.act(cx, move |engine| engine.dsp_apply_preset(device, name));
    }

    pub fn set_filters_only(&mut self, only: bool, device: String, cx: &mut Context<Self>) {
        self.act(cx, move |engine| engine.dsp_set_filters_only(device, only));
    }

    /// Saves a step of an edit made on a graph or in the band table. One
    /// save of an edit is under way at a time, since each names the filter
    /// as the one before it stored it; a newer step waits and is sent once
    /// that lands, and steps in between are dropped. Steps in the middle of
    /// a drag play without moving `version`, so the page does not reload
    /// under the pointer; the `last` one moves it. Held here rather than by
    /// the view, so the last step of a page closed under a drag still lands.
    pub fn edit(
        &mut self,
        edit: u64,
        step: Commit,
        last: bool,
        done: Done,
        cx: &mut Context<Self>,
    ) {
        // An edit refused as stale stays closed: what the rest of its drag
        // sends names a filter that is gone. Its owner still hears so.
        if self.closed.contains(&edit) {
            self.read(false, cx, move |stamp, cx| {
                done(Outcome::Changed.at(stamp, None), cx)
            });
            return;
        }
        let stored = self.stored.remove(&edit);
        let lane = self.lanes.entry(edit).or_insert_with(|| Lane {
            expected: stored.unwrap_or_else(|| step.expected().clone()),
            saving: false,
            next: None,
        });
        if lane.saving {
            let last = last || lane.next.as_ref().is_some_and(|(_, l, _)| *l);
            lane.next = Some((step, last, done));
            return;
        }
        lane.saving = true;
        let step = step.expecting(lane.expected.clone());
        *self.edits.entry(step.name().to_owned()).or_default() += 1;
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let stored = match step {
                Commit::Band {
                    name,
                    index,
                    expected,
                    kind,
                    freq,
                    gain_db,
                    q,
                } => {
                    engine
                        .dsp_edit_band(name, index, expected, kind, freq, gain_db, q)
                        .await
                }
                Commit::Curve {
                    name,
                    index,
                    expected,
                    points,
                } => engine.dsp_edit_curve(name, index, expected, points).await,
            };
            this.update(cx, |dsp, cx| dsp.landed(edit, stored, last, done, cx))
                .ok();
        })
        .detach();
    }

    fn landed(
        &mut self,
        edit: u64,
        stored: Result<DspEdited, KoanError>,
        last: bool,
        done: Done,
        cx: &mut Context<Self>,
    ) {
        let Some(lane) = self.lanes.get_mut(&edit) else {
            return;
        };
        lane.saving = false;
        let outcome = match stored {
            Ok(DspEdited::Stored { band, .. }) => {
                lane.expected = band;
                if let Some((step, last, done)) = lane.next.take() {
                    self.edit(edit, step, last, done, cx);
                    return;
                }
                if !last {
                    cx.notify();
                    return;
                }
                self.last_error = None;
                Outcome::Saved
            }
            Ok(DspEdited::Changed) => {
                self.last_error = Some(
                    "The filters changed since they were shown, so that edit was not saved. They are shown again as they are now."
                        .into(),
                );
                Outcome::Changed
            }
            Err(e) => {
                self.last_error = Some(describe(&e));
                Outcome::Failed
            }
        };
        let lane = self.lanes.remove(&edit);
        // A step queued behind a save that was refused or failed is not
        // sent; its owner, waiting on its newest step, hears the outcome.
        let queued = lane
            .as_ref()
            .and_then(|lane| lane.next.as_ref())
            .map(|(_, _, done)| done.clone());
        let band = lane
            .filter(|_| outcome == Outcome::Saved)
            .map(|lane| lane.expected);
        // Forgotten while this save was in flight: nothing more of it is
        // kept.
        if !self.forgotten.remove(&edit) {
            match &band {
                // An edit that goes on after this save, such as a row of the
                // band table still being typed in, names what it stored.
                Some(stored) => {
                    self.stored.insert(edit, stored.clone());
                }
                None => {
                    self.closed.insert(edit);
                }
            }
        }
        self.read(true, cx, move |stamp, cx| {
            let ended = outcome.at(stamp, band);
            if let Some(queued) = queued {
                queued(ended.clone(), cx);
            }
            done(ended, cx)
        });
    }

    /// The edit `edit` is over, as its owner heard: nothing more of it
    /// will be sent.
    pub fn forget(&mut self, edit: u64) {
        self.closed.remove(&edit);
        self.stored.remove(&edit);
        if self.lanes.contains_key(&edit) {
            self.forgotten.insert(edit);
        }
    }

    /// A new edit's number, for `edit` to keep its saves in order by.
    pub fn next_edit(&mut self) -> u64 {
        self.edit_count += 1;
        self.edit_count
    }

    /// How many edits have been made here to `name`: a page that has seen
    /// none since it last looked takes a change to it as another device's.
    pub fn edits(&self, name: &str) -> u64 {
        self.edits.get(name).copied().unwrap_or(0)
    }

    pub fn delete(&mut self, name: String, cx: &mut Context<Self>) {
        self.act(cx, move |engine| engine.dsp_delete(name));
    }

    pub fn revert(&mut self, name: String, cx: &mut Context<Self>) {
        self.act(cx, move |engine| engine.dsp_revert(name));
    }
}

/// What an edit is told when its saves end.
pub type Done = Rc<dyn Fn(Ended, &mut App)>;

/// The saves of one edit: the filter as last stored, whether a save is
/// under way, and the newest step waiting for it.
struct Lane {
    expected: DspBand,
    saving: bool,
    next: Option<(Commit, bool, Done)>,
}

/// How an edit ended, and the stamp of the overview read after it: a
/// response read at or after it shows the edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Saved,
    /// The filter was no longer the one the edit started on.
    Changed,
    Failed,
}

impl Outcome {
    fn at(self, stamp: (u64, u64), band: Option<DspBand>) -> Ended {
        Ended {
            outcome: self,
            stamp,
            band,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ended {
    pub outcome: Outcome,
    pub stamp: (u64, u64),
    /// Saved, the filter as stored: held to the ranges, which is what the
    /// editor shows next.
    pub band: Option<DspBand>,
}

/// A step of an edit made on a graph or in the band table, to save.
#[derive(Clone, Debug)]
pub enum Commit {
    Band {
        name: String,
        index: u32,
        expected: DspBand,
        kind: String,
        freq: f64,
        gain_db: f64,
        q: f64,
    },
    Curve {
        name: String,
        index: u32,
        expected: DspBand,
        points: Vec<DspPoint>,
    },
}

impl Commit {
    pub fn name(&self) -> &str {
        match self {
            Commit::Band { name, .. } | Commit::Curve { name, .. } => name,
        }
    }

    pub fn expected(&self) -> &DspBand {
        match self {
            Commit::Band { expected, .. } | Commit::Curve { expected, .. } => expected,
        }
    }

    /// The same step, expecting the filter as `stored` holds it.
    pub fn expecting(mut self, stored: DspBand) -> Self {
        match &mut self {
            Commit::Band { expected, .. } | Commit::Curve { expected, .. } => *expected = stored,
        }
        self
    }
}

/// An engine error as a sentence for a person.
pub fn describe(e: &KoanError) -> SharedString {
    match e {
        KoanError::Database { message }
        | KoanError::Audio { message }
        | KoanError::Player { message }
        | KoanError::NotFound { message }
        | KoanError::BadArgument { message }
        | KoanError::Remote { message }
        | KoanError::Refused { message }
        | KoanError::NeedsSampleRate { message } => message.clone().into(),
    }
}

/// What an import did, in a sentence or two: nothing to say for one file
/// made into one profile.
pub fn describe_import(summary: &DspImportSummary, files: usize) -> Option<String> {
    let mut lines = Vec::new();
    if let Some(group) = &summary.group {
        let playing = summary
            .imported
            .first()
            .map(|first| {
                format!(
                    " “{first}” is playing; pick another on the group's page or in an output's preset menu."
                )
            })
            .unwrap_or_default();
        lines.push(format!(
            "Imported {} presets as the group “{group}”.{playing}",
            summary.imported.len()
        ));
    } else if files > 1
        && let Some(one) = summary.imported.first()
    {
        lines.push(format!("Combined {files} files into “{one}”."));
    }
    if !summary.refused.is_empty() {
        let why: Vec<String> = summary
            .refused
            .iter()
            .map(|r| format!("{}: {}", r.file, r.reason))
            .collect();
        lines.push(format!(
            "{} refused: {}",
            summary.refused.len(),
            why.join("; ")
        ));
    }
    lines.extend(summary.notes.iter().cloned());
    (!lines.is_empty()).then(|| lines.join(" "))
}

/// What a profile holds, in a line.
pub fn describe_profile(p: &DspProfileSummary) -> String {
    let mut parts: Vec<String> = Vec::new();
    if p.measured {
        parts.push("From a measurement".into());
    }
    if !p.members.is_empty() {
        parts.push(format!("{} to pick from", p.members.len()));
    } else if p.preset {
        parts.push(plural(p.layers, "part", "parts"));
    } else if p.layers > 0 {
        parts.push(format!("Plays {} in order", plural(p.layers, "EQ", "EQs")));
    }
    // A graphic curve is one filter of many points, as its page lists it.
    if p.graphics > 0 {
        parts.push(format!("graphic, {}", plural(p.points, "point", "points")));
    }
    let bands = p.bands.saturating_sub(p.graphics);
    if bands > 0 {
        parts.push(plural(bands, "band", "bands"));
    }
    if !p.rates.is_empty() {
        let rates: Vec<String> = p.rates.iter().map(|r| khz(*r)).collect();
        parts.push(format!("{} kHz", rates.join(", ")));
    }
    parts.join(" · ")
}

fn plural(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A rate in kHz, as a person writes it: 48, 44.1.
pub fn khz(hz: u32) -> String {
    if hz.is_multiple_of(1000) {
        format!("{}", hz / 1000)
    } else {
        format!("{:.1}", hz as f64 / 1000.0)
    }
}

/// A list as a sentence: "a", "a and b", "a, b and c".
pub fn joined(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_read_as_people_write_them() {
        assert_eq!(khz(48000), "48");
        assert_eq!(khz(44100), "44.1");
        assert_eq!(khz(176_400), "176.4");
    }

    #[test]
    fn lists_read_as_sentences() {
        let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(joined(&s(&[])), "");
        assert_eq!(joined(&s(&["a"])), "a");
        assert_eq!(joined(&s(&["a", "b"])), "a and b");
        assert_eq!(joined(&s(&["a", "b", "c"])), "a, b and c");
    }
}
