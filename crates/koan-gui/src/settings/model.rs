//! The settings window's copy of the configuration, and what Settings does
//! to the engine.
//!
//! `config.toml` is shared with the CLI and the TUI, so this is a view onto
//! that file rather than a second source of truth: a control commits when it
//! is changed, and the window reads the file again when it comes back to the
//! front, so a change made elsewhere is not silently overwritten.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use std::rc::Rc;

use koan_kit::app::{App, Context, Entity, SharedString, Subscription, Task};
use koan_runtime::{KoanEngine, KoanError, Settings};

use crate::mirror::{Mirror, Slice};

/// What a task holds while it runs, so another that needs the same waits.
/// The Mac app's `ActivityModel.Resources`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Uses(u8);

impl Uses {
    pub const NONE: Uses = Uses(0);
    /// The files on disk: read for their tags, or moved.
    pub const LOCAL_FILES: Uses = Uses(1);
    /// Library rows for files on this machine.
    pub const LOCAL_TRACKS: Uses = Uses(1 << 1);
    /// Library rows mirrored from the server.
    pub const REMOTE_TRACKS: Uses = Uses(1 << 2);
    /// The downloaded copies of remote tracks.
    pub const DOWNLOADS: Uses = Uses(1 << 3);
    /// Reading files and writing their rows: a scan.
    pub const LOCAL_LIBRARY: Uses = Uses(Self::LOCAL_FILES.0 | Self::LOCAL_TRACKS.0);
    /// Emptying the library.
    pub const WHOLE_LIBRARY: Uses = Uses(0b1111);

    pub const fn with(self, other: Uses) -> Uses {
        Uses(self.0 | other.0)
    }

    pub const fn overlaps(self, other: Uses) -> bool {
        self.0 & other.0 != 0
    }
}

/// A change to the settings. Applied to the copy drawn at once, and again
/// to the file's current contents when it is written, so it must give the
/// same result applied twice: an append checks the item is not there.
pub type Edit = Rc<dyn Fn(&mut Settings)>;

/// The settings as the file last read, and the edits not yet in it.
///
/// A write reads the file, applies the edits it carries and writes the
/// result, so it never writes a field nobody here touched: a server signed
/// in to by the engine, or a folder the TUI added, stays as it is. One write
/// runs at a time; edits made meanwhile wait for the next.
pub(crate) struct Draft {
    base: Settings,
    writing: Vec<Edit>,
    queued: Vec<Edit>,
    /// Writes landed so far. A read of the file that began before the last
    /// of them may have read the file before it, and is not applied.
    writes: u64,
    /// A write in the chain under way failed, though a later one may have
    /// landed.
    chain_failed: bool,
}

impl Draft {
    pub(crate) fn new(base: Settings) -> Self {
        Self {
            base,
            writing: Vec::new(),
            queued: Vec::new(),
            writes: 0,
            chain_failed: false,
        }
    }

    /// Nothing is being written or waiting to be.
    pub(crate) fn settled(&self) -> bool {
        self.writing.is_empty() && self.queued.is_empty()
    }

    /// Writes landed so far, for a read to say when it began.
    pub(crate) fn writes(&self) -> u64 {
        self.writes
    }

    /// What the window shows: the file with every edit not yet in it.
    pub(crate) fn drawn(&self) -> Settings {
        let mut drawn = self.base.clone();
        for edit in self.writing.iter().chain(&self.queued) {
            edit(&mut drawn);
        }
        drawn
    }

    /// Queues an edit. True when no write is running, so one should start.
    pub(crate) fn edit(&mut self, edit: Edit) -> bool {
        self.queued.push(edit);
        self.writing.is_empty()
    }

    /// The edits the next write carries, now counted as being written.
    pub(crate) fn take(&mut self) -> Vec<Edit> {
        self.writing = std::mem::take(&mut self.queued);
        self.writing.clone()
    }

    /// A write landed: `written` is what it wrote. True when edits are
    /// waiting for another.
    pub(crate) fn wrote(&mut self, written: Settings) -> bool {
        self.base = written;
        self.writing.clear();
        self.writes += 1;
        !self.queued.is_empty()
    }

    /// A write failed: its edits are dropped, so the window shows what the
    /// file holds. True when edits are waiting for another.
    pub(crate) fn failed(&mut self) -> bool {
        self.writing.clear();
        self.chain_failed = true;
        !self.queued.is_empty()
    }

    /// The chain of writes is done, nothing waiting: true when every write
    /// in it landed, so what waited on it may run.
    pub(crate) fn chain_done(&mut self) -> bool {
        !std::mem::take(&mut self.chain_failed)
    }

    /// The file read again, by a read that began after `writes` writes had
    /// landed: the edits not yet in it still show over it. A read that began
    /// before a write since landed is dropped; it may hold the file as it
    /// was before that write.
    pub(crate) fn reloaded(&mut self, base: Settings, writes: u64) {
        if writes == self.writes {
            self.base = base;
        }
    }
}

/// Work that waits for the writes under way to land.
type Then = Box<dyn FnOnce(&mut SettingsModel, &mut Context<SettingsModel>)>;
/// What runs once the file has first been read.
type Loaded = Box<dyn FnOnce(&mut App)>;

/// What a sign-in is called while it runs, by which the pane knows one is.
pub const SIGNING_IN: &str = "Signing in";

/// kōan's public demonstration server, and the account open to anyone.
pub const DEMO: (&str, &str, &str) = ("https://demo.koan.rocks", "demo", "demo");

/// How long the result of an action stays on the status line. An error
/// stays until the next action.
const RESULT_SHOWN: Duration = Duration::from_secs(6);

pub struct SettingsModel {
    engine: Arc<KoanEngine>,
    /// `None` until the file has first been read.
    draft: Option<Draft>,
    /// The draft as drawn, worked out when it changes rather than per frame.
    shown: Option<Settings>,
    /// Edits made before the file was first read, applied once it has been.
    early: Vec<Edit>,
    /// What waits for the file's first read: a link that launched the app
    /// is answered as the settings stand, not as signed out.
    loaded: Vec<Loaded>,
    /// What waits for the writes under way to land.
    written: Vec<Then>,
    /// The error shown is a write's.
    write_failed: bool,
    pub last_error: Option<SharedString>,
    pub last_result: Option<SharedString>,
    dismissal: Option<Task<()>>,
    reading: Option<Task<()>>,
    /// The tasks Settings started: what each holds, and what it is doing,
    /// for the status line.
    running: Vec<(u64, Uses, SharedString)>,
    next_task: u64,
    /// What koan is doing on its own (a watched folder's scan, an automatic
    /// sync), from the engine's tasks slice.
    engine_busy: Uses,
    /// What a task waiting for the edits before it to land will hold, from
    /// when it is asked for: a reset, which clicked again in that gap would
    /// start twice.
    awaiting_write: Uses,
    /// A key or app password made as its window closed, shown by the next
    /// Settings window: it is shown only once, and must reach someone.
    pub unseen: Vec<crate::settings::sheet::Sheet>,
    /// Pairings asked about and not yet answered, declined if the app quits
    /// first.
    pub waiting_pairs: Vec<String>,
    _tasks: Subscription,
}

impl SettingsModel {
    /// The app's one model, reading the file now. It outlives every Settings
    /// window, so an action started from one finishes after it closes.
    pub fn new(engine: Arc<KoanEngine>, cx: &mut Context<Self>) -> Self {
        let tasks = cx.global::<Mirror>().tasks.clone();
        let mut model = Self {
            engine,
            draft: None,
            shown: None,
            early: Vec::new(),
            loaded: Vec::new(),
            written: Vec::new(),
            write_failed: false,
            last_error: None,
            last_result: None,
            dismissal: None,
            reading: None,
            running: Vec::new(),
            next_task: 0,
            engine_busy: Uses::NONE,
            awaiting_write: Uses::NONE,
            unseen: Vec::new(),
            waiting_pairs: Vec::new(),
            _tasks: cx.observe(&tasks, |model, tasks, cx| {
                let Slice(tasks) = *tasks.read(cx);
                let busy = Uses::NONE
                    .with(if tasks.scanning {
                        Uses::LOCAL_LIBRARY
                    } else {
                        Uses::NONE
                    })
                    .with(if tasks.syncing {
                        Uses::REMOTE_TRACKS
                    } else {
                        Uses::NONE
                    });
                if busy != model.engine_busy {
                    model.engine_busy = busy;
                    cx.notify();
                }
            }),
        };
        model.reload(cx);
        model
    }

    pub fn engine(&self) -> &Arc<KoanEngine> {
        &self.engine
    }

    /// The settings as drawn: the file, with every edit not yet written.
    pub fn settings(&self) -> Option<&Settings> {
        self.shown.as_ref()
    }

    /// Whether starting something that needs `wanted` would collide with
    /// what is running.
    pub fn conflicts(&self, wanted: Uses) -> bool {
        self.running
            .iter()
            .fold(
                self.engine_busy.with(self.awaiting_write),
                |held, (_, uses, _)| held.with(*uses),
            )
            .overlaps(wanted)
    }

    /// What Settings has under way, as the status line says it.
    pub fn running(&self) -> impl Iterator<Item = &SharedString> {
        self.running.iter().map(|(_, _, label)| label)
    }

    // --- Reading and writing ---------------------------------------------

    /// Reads the file again. Edits not yet written still show over it.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let engine = self.engine.clone();
        let writes = self.draft.as_ref().map_or(0, Draft::writes);
        self.reading = Some(cx.spawn(async move |this, cx| {
            let settings = engine.settings().await;
            this.update(cx, |model, cx| {
                match &mut model.draft {
                    Some(draft) => draft.reloaded(settings, writes),
                    None => {
                        // Edits made before the file was first read go in now.
                        let mut draft = Draft::new(settings);
                        let early = std::mem::take(&mut model.early);
                        let mut start = false;
                        for edit in early {
                            // Every edit queued, whatever the first answered.
                            start |= draft.edit(edit);
                        }
                        model.draft = Some(draft);
                        if start {
                            model.write(cx);
                        }
                        for then in std::mem::take(&mut model.loaded) {
                            cx.defer(then);
                        }
                    }
                }
                // Kept running as the file says now, edited here or by
                // the terminal UI or by hand.
                if let Some(shown) = model.draft.as_ref().map(Draft::drawn) {
                    crate::platform::residency::follow_file(shown.devices_keep_running, cx);
                    crate::platform::update::follow(shown.updates_auto, cx);
                }
                model.redraw(cx);
            })
            .ok();
        }));
    }

    fn redraw(&mut self, cx: &mut Context<Self>) {
        self.shown = self.draft.as_ref().map(Draft::drawn);
        cx.notify();
    }

    /// Changes fields and writes them. Every control commits through here,
    /// so there is one place that decides when the file is touched. `change`
    /// runs twice, on the copy drawn and on the file as it stands when it is
    /// written: an append checks the item is not there already.
    pub fn edit(&mut self, cx: &mut Context<Self>, change: impl Fn(&mut Settings) + 'static) {
        let Some(draft) = &mut self.draft else {
            self.early.push(Rc::new(change));
            return;
        };
        let start = draft.edit(Rc::new(change));
        self.redraw(cx);
        if start {
            self.write(cx);
        }
    }

    /// Runs `then` once the file has been read: now, if it has. Outside the
    /// model's update, so it may update the model and its windows.
    pub(crate) fn when_loaded(
        &mut self,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut App) + 'static,
    ) {
        if self.draft.is_some() {
            cx.defer(then);
        } else {
            self.loaded.push(Box::new(then));
        }
    }

    /// Runs `then` once the edits made so far are in the file: now, if none
    /// are waiting.
    pub(crate) fn when_written(
        &mut self,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
    ) {
        let settled = self.early.is_empty() && self.draft.as_ref().is_some_and(Draft::settled);
        if settled {
            then(self, cx);
        } else {
            self.written.push(Box::new(then));
        }
    }

    /// Writes the queued edits over the file as it stands, then any queued
    /// meanwhile, until none are left.
    fn write(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = &mut self.draft else { return };
        let edits = draft.take();
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let read = engine.clone().settings().await;
            let mut settings = read.clone();
            for edit in &edits {
                edit(&mut settings);
            }
            // Only what the edits changed is written, so a sign-in or another
            // front end's write landing after the read is not put back.
            let result = engine.update_settings_since(read, settings.clone()).await;
            this.update(cx, |model, cx| {
                let Some(draft) = &mut model.draft else {
                    return;
                };
                let more = match result {
                    Ok(()) => {
                        // Only a write's own failure is cleared by the next
                        // write that lands; another action's stays.
                        if std::mem::take(&mut model.write_failed) {
                            model.last_error = None;
                        }
                        draft.wrote(settings)
                    }
                    Err(e) => {
                        model.last_error = Some(describe(&e).into());
                        model.write_failed = true;
                        draft.failed()
                    }
                };
                model.redraw(cx);
                if more {
                    model.write(cx);
                } else {
                    // What waited on a write that failed does not run, even
                    // where a later write in the chain landed: a scan of a
                    // folder that was not saved would clear the error saying
                    // so.
                    let whole = model.draft.as_mut().is_none_or(Draft::chain_done);
                    let waiting = std::mem::take(&mut model.written);
                    if !whole {
                        model.awaiting_write = Uses::NONE;
                    }
                    if whole {
                        for then in waiting {
                            then(model, cx);
                        }
                    }
                    // What was written is the read with the edits over it,
                    // and only the changed fields went to the file: a sign-in
                    // landing between the read and the write is in the file
                    // and not in the draft. Read once the chain is done, so
                    // no write landing after it drops the read.
                    model.reload(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    // --- The status line ---------------------------------------------------

    pub fn report(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.last_error = Some(message.into());
        cx.notify();
    }

    /// A result, shown for a few seconds. A later one replaces it, and the
    /// timer goes with the result it was started for.
    pub fn done(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.last_error = None;
        self.last_result = Some(message.into());
        cx.notify();
        self.dismissal = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RESULT_SHOWN).await;
            this.update(cx, |model, cx| {
                model.last_result = None;
                cx.notify();
            })
            .ok();
        }));
    }

    // --- Running engine work -----------------------------------------------

    /// Runs `work` holding `uses`, then hands its result to `landed` and reads
    /// the file again, however it ended. The registration goes whether the
    /// work succeeds or fails, so nothing stays disabled.
    pub fn run<T: 'static, F>(
        &mut self,
        label: impl Into<SharedString>,
        uses: Uses,
        cx: &mut Context<Self>,
        work: impl FnOnce(Arc<KoanEngine>) -> F,
        landed: impl FnOnce(&mut Self, Result<T, KoanError>, &mut Context<Self>) + 'static,
    ) where
        F: Future<Output = Result<T, KoanError>> + 'static,
    {
        let id = self.next_task;
        self.next_task += 1;
        // A result stays until the next action, an error too.
        self.last_error = None;
        self.write_failed = false;
        self.last_result = None;
        self.dismissal = None;
        self.running.push((id, uses, label.into()));
        cx.notify();
        let future = work(self.engine.clone());
        cx.spawn(async move |this, cx| {
            let result = future.await;
            this.update(cx, |model, cx| {
                model.running.retain(|(task, _, _)| *task != id);
                landed(model, result, cx);
                model.reload(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn scan(&mut self, force: bool, cx: &mut Context<Self>) {
        self.run(
            if force {
                "Rescanning every file"
            } else {
                "Scanning the library"
            },
            Uses::LOCAL_LIBRARY,
            cx,
            move |engine| engine.scan_reporting(force, None),
            |model, result, cx| match result {
                Ok(s) => model.done(
                    format!(
                        "{} added · {} updated · {} removed",
                        s.added, s.updated, s.removed
                    ),
                    cx,
                ),
                Err(e) => model.report(describe(&e), cx),
            },
        );
    }

    /// Adds folders to scan, leaving out any already listed, and scans.
    pub fn add_folders(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        self.edit(cx, move |s| {
            for path in &paths {
                if !s.library_folders.iter().any(|f| &f.path == path) {
                    // The scan this starts fills the count in.
                    s.library_folders.push(koan_runtime::LibraryFolder {
                        path: path.clone(),
                        tracks: 0,
                    });
                }
            }
        });
        // The scan reads the folders from the file, so it waits for them.
        self.when_written(cx, |model, cx| model.scan(false, cx));
    }

    /// Stops scanning a folder, and with `forget`, removes what it put in the
    /// library. Its files are not touched either way.
    pub fn remove_folder(&mut self, path: String, forget: bool, cx: &mut Context<Self>) {
        let removed = path.clone();
        self.edit(cx, move |s| s.library_folders.retain(|f| f.path != removed));
        if !forget {
            return;
        }
        let name = std::path::Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        self.run(
            format!("Forgetting {name}"),
            Uses::LOCAL_TRACKS,
            cx,
            move |engine| engine.forget_folder(path),
            |model, result, cx| match result {
                Ok(n) => {
                    // Their album ids go to whatever is scanned next.
                    crate::covers::Covers::purge(cx);
                    model.done(format!("Forgot {} tracks", count(n)), cx);
                }
                Err(e) => model.report(describe(&e), cx),
            },
        );
    }

    pub fn rebuild_index(&mut self, cx: &mut Context<Self>) {
        self.run(
            "Rebuilding the library index",
            Uses::WHOLE_LIBRARY,
            cx,
            |engine| engine.rebuild_index(),
            |model, result, cx| match result {
                // The rows stay, and their sleeves with them: what the next
                // scan finds changed it says through `art_evictions`.
                Ok(s) => model.done(
                    format!(
                        "{} tracks to read again — scan or sync to rebuild",
                        count(s.tracks)
                    ),
                    cx,
                ),
                Err(e) => model.report(describe(&e), cx),
            },
        );
    }

    /// Signs in with a password or an API key, then loads the library.
    pub fn sign_in(
        &mut self,
        url: String,
        username: String,
        secret: String,
        with_key: bool,
        cx: &mut Context<Self>,
    ) {
        let shown = url.clone();
        self.run(
            SIGNING_IN,
            Uses::NONE,
            cx,
            move |engine| async move {
                if with_key {
                    engine.sign_in_remote_with_key(url, username, secret).await
                } else {
                    engine.sign_in_remote(url, username, secret).await
                }
            },
            move |model, result, cx| match result {
                Ok(()) => {
                    // Another server's library replaces the one before, and
                    // takes its album ids.
                    crate::covers::Covers::purge(cx);
                    model.done(format!("Signed in to {shown}"), cx);
                    model.load_library(cx);
                }
                Err(e) => model.report(describe(&e), cx),
            },
        );
    }

    /// Whether a sign-in is under way.
    pub fn signing_in(&self) -> bool {
        self.running().any(|label| label.as_ref() == SIGNING_IN)
    }

    /// Signs in to the demonstration server, unless a sign-in is under way.
    pub fn sign_in_demo(&mut self, cx: &mut Context<Self>) {
        if self.signing_in() {
            return;
        }
        let (url, username, password) = DEMO;
        self.sign_in(url.into(), username.into(), password.into(), false, cx);
    }

    /// Signs in through an invite, then loads the library.
    pub fn join_invite(&mut self, invite: koan_runtime::Invite, cx: &mut Context<Self>) {
        let host = invite.server.clone();
        self.run(
            SIGNING_IN,
            Uses::NONE,
            cx,
            move |engine| engine.join_invite(invite),
            move |model, result, cx| match result {
                Ok(()) => {
                    crate::covers::Covers::purge(cx);
                    model.done(format!("Signed in to {host}"), cx);
                    model.load_library(cx);
                }
                Err(e) => {
                    model.report(format!("Could not sign in to {host}: {}", describe(&e)), cx)
                }
            },
        );
    }

    fn load_library(&mut self, cx: &mut Context<Self>) {
        self.run(
            "Loading the library",
            Uses::REMOTE_TRACKS,
            cx,
            |engine| engine.load_remote_library(),
            |model, result, cx| {
                if let Err(e) = result {
                    model.report(
                        format!("Signed in, but the library did not load: {}", describe(&e)),
                        cx,
                    );
                }
            },
        );
    }

    /// Signs out and forgets everything held for the server: its library,
    /// the account's favourites, history and playlists, and the downloads.
    /// Local files stay.
    pub fn sign_out(&mut self, cx: &mut Context<Self>) {
        self.run(
            "Signing out",
            // Local rows too: it takes the server off the rows that were on
            // both, so a scan writing them would be writing the same rows.
            Uses::REMOTE_TRACKS.with(Uses::LOCAL_TRACKS),
            cx,
            |engine| engine.sign_out_remote(),
            |model, result, cx| match result {
                Ok(()) => {
                    // The server's records are gone, and their ids go to
                    // whatever is synced next.
                    crate::covers::Covers::purge(cx);
                    model.done("Signed out", cx);
                }
                Err(e) => model.report(describe(&e), cx),
            },
        );
    }

    /// Puts kōan back as it was first opened: stops sharing, signs out
    /// (forgetting the server's library and the account's favourites,
    /// history and playlists here), forgets each folder's tracks, and sets
    /// every setting back to its default. Starts once the edits still being
    /// written have landed, so none lands over the defaults; `after` runs
    /// once it has succeeded.
    pub fn reset(
        &mut self,
        sharing: Entity<crate::settings::sharing::Sharing>,
        after: impl FnOnce(&mut App) + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(settings) = self.settings().cloned() else {
            return;
        };
        let folders: Vec<String> = settings
            .library_folders
            .iter()
            .map(|f| f.path.clone())
            .collect();
        let listed = settings.library_folders.clone();
        let signed_in = settings.remote_signed_in || settings.remote_enabled;
        let host = crate::settings::panes::server::host(&settings.remote_url);
        // The folders leave the file before their tracks are forgotten, so
        // the folder watcher has nothing to index them again from.
        self.awaiting_write = Uses::WHOLE_LIBRARY;
        self.edit(cx, |s| s.library_folders.clear());
        self.when_written(cx, move |model, cx| {
            model.awaiting_write = Uses::NONE;
            model.run(
                "Resetting kōan",
                Uses::WHOLE_LIBRARY,
                cx,
                move |engine| async move {
                    // Sharing reads what is running once this lands.
                    engine.clone().stop_server().await;
                    if signed_in {
                        engine.clone().sign_out_remote().await?;
                    }
                    for path in folders {
                        engine.clone().forget_folder(path).await?;
                    }
                    engine.reset_settings().await
                },
                move |model, result, cx| {
                    sharing.update(cx, |sharing, cx| sharing.refresh(cx));
                    match result {
                        Ok(()) => {
                            crate::covers::Covers::purge(cx);
                            model.done("Every setting is back to its default", cx);
                            cx.defer(after);
                        }
                        Err(e) => {
                            // Stopped part way: sharing is stopped and may be
                            // signed out of, so the file says so; the
                            // folders go back in the list, and a scan reads
                            // again what was forgotten.
                            let failed = describe(&e);
                            model.edit(cx, move |s| {
                                s.share_enabled = false;
                                for folder in &listed {
                                    if !s.library_folders.iter().any(|f| f.path == folder.path) {
                                        s.library_folders.push(folder.clone());
                                    }
                                }
                            });
                            let signed_out = if signed_in {
                                format!(" kōan may already be signed out of {host}.")
                            } else {
                                String::new()
                            };
                            model.report(
                                format!("{failed}. The reset stopped part way: sharing is off, and the folders are back in the list.{signed_out} Reset again to finish it."),
                                cx,
                            );
                        }
                    }
                },
            );
        });
    }

    pub fn sync(&mut self, cx: &mut Context<Self>) {
        let activity = cx
            .global::<crate::library::activity::ActivityHandle>()
            .0
            .clone();
        activity.update(cx, |a, cx| a.follow_sync(true, cx));
        self.run(
            "Syncing with the server",
            Uses::REMOTE_TRACKS,
            cx,
            |engine| engine.sync_remote(),
            move |model, result, cx| {
                activity.update(cx, |a, cx| a.follow_sync(false, cx));
                match result {
                    // "0 tracks across 0 albums" reads as a failure, and is what
                    // an empty server answers.
                    Ok(s) if s.tracks == 0 && s.favourites_imported == 0 => {
                        model.done("Already up to date", cx)
                    }
                    Ok(s) => model.done(
                        format!(
                            "{} tracks across {} albums",
                            count(s.tracks.into()),
                            count(s.albums.into())
                        ),
                        cx,
                    ),
                    Err(e) => model.report(describe(&e), cx),
                }
            },
        );
    }

    pub fn clear_cache(&mut self, cx: &mut Context<Self>) {
        self.run(
            "Clearing downloads",
            Uses::DOWNLOADS,
            cx,
            |engine| engine.clear_download_cache(),
            |model, result, cx| match result {
                Ok(c) => model.done(
                    format!("Freed {} across {} files", bytes(c.bytes), count(c.files)),
                    cx,
                ),
                Err(e) => model.report(describe(&e), cx),
            },
        );
    }

    /// Keeps this device's queue on the server, or stops. Turning it on
    /// replaces this queue with the server's, which the pane asks about first.
    pub fn set_server_queue(&mut self, on: bool, cx: &mut Context<Self>) {
        self.run(
            if on {
                "Taking the server's queue"
            } else {
                "Keeping the queue here"
            },
            Uses::NONE,
            cx,
            move |engine| engine.set_server_queue(on),
            |model, result, cx| match result {
                Ok(()) => {
                    model.last_error = None;
                    cx.notify();
                }
                Err(e) => model.report(describe(&e), cx),
            },
        );
    }

    /// Changes the signed-in account's password. This device stays signed
    /// in; the account's others sign in again.
    pub fn change_password(&mut self, current: String, new: String, cx: &mut Context<Self>) {
        self.run(
            "Changing the password",
            Uses::NONE,
            cx,
            move |engine| engine.change_own_password(current, new),
            |model, result, cx| match result {
                Ok(()) => model.done(
                    "Password changed. Your other devices will have to sign in again.",
                    cx,
                ),
                Err(e) => model.report(describe(&e), cx),
            },
        );
    }
}

/// An engine error's own message, which is worth reading.
pub fn describe(error: &KoanError) -> String {
    match error {
        KoanError::BadArgument { message }
        | KoanError::Database { message }
        | KoanError::NotFound { message }
        | KoanError::Audio { message }
        | KoanError::Player { message }
        | KoanError::Remote { message }
        | KoanError::Refused { message } => message.clone(),
        other => other.to_string(),
    }
}

/// A count with thousands grouped, as the Mac formats one.
pub fn count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

pub use crate::time::bytes;

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(port: u16) -> Settings {
        Settings {
            library_folders: Vec::new(),
            remote_enabled: false,
            remote_url: String::new(),
            remote_username: String::new(),
            remote_signed_in: false,
            remote_tracks: 0,
            remote_last_sync: None,
            download_workers: 4,
            cache_limit: String::new(),
            download_ahead: Some(2),
            preload_covers: false,
            cache_dir: String::new(),
            cache_bytes: 0,
            auto_sync: false,
            auto_sync_interval_mins: 0,
            play_queue: false,
            replaygain: "off".into(),
            pre_amp_db: 0.0,
            fade_on_pause: false,
            devices_discoverable: true,
            devices_addresses: Vec::new(),
            devices_refused: Vec::new(),
            devices_nearby_control: "full".into(),
            devices_keep_running: false,
            updates_auto: true,
            share_enabled: false,
            share_port: port,
            share_invite_to: koan_runtime::InviteTo::Network,
            server_url: String::new(),
        }
    }

    fn add(path: &'static str) -> Edit {
        Rc::new(move |s: &mut Settings| {
            if !s.library_folders.iter().any(|f| f.path == path) {
                s.library_folders.push(koan_runtime::LibraryFolder {
                    path: path.into(),
                    tracks: 0,
                });
            }
        })
    }

    fn paths(s: &Settings) -> Vec<&str> {
        s.library_folders.iter().map(|f| f.path.as_str()).collect()
    }

    #[test]
    fn one_write_at_a_time_carrying_what_was_queued() {
        let mut draft = Draft::new(settings(1));
        assert!(draft.edit(Rc::new(|s| s.share_port = 2)));
        let first = draft.take();
        assert_eq!(first.len(), 1);
        // Made while the first is written: drawn at once, written next.
        assert!(!draft.edit(Rc::new(|s| s.share_port = 3)));
        assert_eq!(draft.drawn().share_port, 3);
        let mut written = settings(1);
        first.iter().for_each(|e| e(&mut written));
        assert!(draft.wrote(written));
        assert_eq!(draft.take().len(), 1);
        assert_eq!(draft.drawn().share_port, 3);
    }

    #[test]
    fn writes_only_what_was_edited() {
        // The engine signed in behind the window: the file has a server the
        // window's copy does not, and the edit leaves it.
        let mut draft = Draft::new(settings(1));
        draft.edit(Rc::new(|s| s.fade_on_pause = true));
        let edits = draft.take();
        let mut file = settings(1);
        file.remote_url = "https://music.example".into();
        edits.iter().for_each(|e| e(&mut file));
        assert_eq!(file.remote_url, "https://music.example");
        assert!(file.fade_on_pause);
    }

    #[test]
    fn an_append_already_in_the_file_is_not_doubled() {
        let mut base = settings(1);
        base.library_folders.push(koan_runtime::LibraryFolder {
            path: "/music".into(),
            tracks: 10,
        });
        let mut draft = Draft::new(base.clone());
        draft.edit(add("/music"));
        assert_eq!(paths(&draft.drawn()), ["/music"]);
        let mut file = base;
        draft.take().iter().for_each(|e| e(&mut file));
        assert_eq!(paths(&file), ["/music"]);
    }

    #[test]
    fn a_failed_write_drops_its_edits_and_keeps_later_ones() {
        let mut draft = Draft::new(settings(1));
        draft.edit(add("/a"));
        draft.take();
        draft.edit(add("/b"));
        assert_eq!(paths(&draft.drawn()), ["/a", "/b"]);
        assert!(draft.failed());
        assert_eq!(paths(&draft.drawn()), ["/b"]);
        draft.take();
        assert!(!draft.failed());
        assert!(paths(&draft.drawn()).is_empty());
    }

    /// A write that failed keeps the chain from counting as written, though
    /// a write queued behind it landed: what waits on the chain (a scan of
    /// an unsaved folder) does not run. The next chain starts clean.
    #[test]
    fn a_failed_write_spoils_its_chain() {
        let mut draft = Draft::new(settings(1));
        draft.edit(add("/a"));
        draft.take();
        draft.edit(add("/b"));
        assert!(draft.failed(), "/b waits");
        let edits = draft.take();
        let mut written = settings(1);
        edits.iter().for_each(|e| e(&mut written));
        assert!(!draft.wrote(written.clone()));
        assert!(!draft.chain_done());
        draft.edit(add("/c"));
        draft.take();
        assert!(!draft.wrote(written));
        assert!(draft.chain_done());
    }

    #[test]
    fn a_read_from_before_a_write_does_not_undo_it() {
        let mut draft = Draft::new(settings(1));
        // A read of the file begins, before the edit is written.
        let began = draft.writes();
        draft.edit(Rc::new(|s| s.share_port = 2));
        let edits = draft.take();
        let mut written = settings(1);
        edits.iter().for_each(|e| e(&mut written));
        assert!(!draft.wrote(written));
        // The read lands with the file as it was.
        draft.reloaded(settings(1), began);
        assert_eq!(draft.drawn().share_port, 2);
        // A read begun after the write is applied.
        let mut later = settings(2);
        later.fade_on_pause = true;
        draft.reloaded(later, draft.writes());
        assert!(draft.drawn().fade_on_pause);
    }

    #[test]
    fn a_reload_keeps_edits_not_yet_written() {
        let mut draft = Draft::new(settings(1));
        draft.edit(Rc::new(|s| s.fade_on_pause = true));
        let mut file = settings(1);
        file.remote_signed_in = true;
        draft.reloaded(file, draft.writes());
        let drawn = draft.drawn();
        assert!(drawn.remote_signed_in);
        assert!(drawn.fade_on_pause);
    }

    #[test]
    fn conflicts_with_what_is_held() {
        assert!(Uses::WHOLE_LIBRARY.overlaps(Uses::DOWNLOADS));
        assert!(Uses::LOCAL_LIBRARY.overlaps(Uses::LOCAL_TRACKS));
        assert!(!Uses::LOCAL_LIBRARY.overlaps(Uses::REMOTE_TRACKS));
        assert!(!Uses::NONE.overlaps(Uses::WHOLE_LIBRARY));
    }

    #[test]
    fn formats_counts_and_sizes() {
        assert_eq!(count(0), "0");
        assert_eq!(count(999), "999");
        assert_eq!(count(1000), "1,000");
        assert_eq!(count(1234567), "1,234,567");
        assert_eq!(bytes(0), "0 bytes");
        assert_eq!(bytes(1), "1 byte");
        assert_eq!(bytes(1500), "1.5 KB");
        assert_eq!(bytes(12_300_000), "12.3 MB");
        assert_eq!(bytes(250_000_000_000), "250 GB");
        assert_eq!(bytes(10_000_000), "10 MB");
        assert_eq!(bytes(999_600_000), "1 GB", "not 1000 MB");
        assert_eq!(bytes(999_600), "1 MB");
    }
}
