//! The EQ page: what a device plays, as the apps' EQ page shows it. Music in,
//! the device's correction, the EQs on top in order, the device out, with
//! the curve of the whole chain above. Each stage opens: the correction to
//! choose another, an EQ to its own page, where its bands are a table to edit
//! from the keyboard. Manage lists every profile there is.
//!
//! What the page shows is read off the frame by `eq_worker`, and every change
//! goes through it too, through the same `profiles` calls the apps and
//! `koan dsp` make, so the three never disagree about what plays.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use koan_core::audio::dsp::profiles::{self, Join, Starter, Summary};
use koan_core::config::{DspFilter, DspRole, DspScope, EqFilterKind};
use koan_core::player::commands::PlayerCommand;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use super::eq_graph::{self, Curve, EqGraph, Handle};
use super::eq_worker::{
    Act, Done, Job, PendingImport, Playing, Reply, RoleAsk, Snapshot, ViewReq, Worker,
};
use super::theme::Theme;

/// A stage of a device's chain, to choose what fills it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Correction,
    Eq,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum View {
    Chain,
    Profile(String),
    Manage,
}

/// A figure of a band's row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Kind,
    Freq,
    Gain,
    Q,
}

const FIELDS: [Field; 4] = [Field::Kind, Field::Freq, Field::Gain, Field::Q];

/// The band types that can be chosen: the config's name and the table's.
const KINDS: [(EqFilterKind, &str); 8] = [
    (EqFilterKind::Peaking, "peak"),
    (EqFilterKind::LowShelf, "low shelf"),
    (EqFilterKind::HighShelf, "high shelf"),
    (EqFilterKind::LowPass, "low pass"),
    (EqFilterKind::HighPass, "high pass"),
    (EqFilterKind::Notch, "notch"),
    (EqFilterKind::BandPass, "band pass"),
    (EqFilterKind::AllPass, "all pass"),
];

/// What a row of a view does when chosen.
#[derive(Debug, Clone, PartialEq)]
enum Row {
    Device,
    Preset,
    Correct,
    SaveOver(String),
    Revert(String),
    SaveNew,
    Correction,
    Eq(usize),
    AddEq,
    Manage,
    // A profile's page.
    Role,
    MadeAgainst,
    CorrectedTo,
    Member,
    Channels,
    Editing,
    Band(usize),
    AddBand,
    Use,
    Sync,
    Rename,
    Copy,
    Reset,
    Delete,
    // Manage.
    Profile(String),
    Import,
    New,
}

/// A line of a view: what it says, and the row it is, if it is one.
struct Line {
    spans: Vec<(String, Style)>,
    right: Vec<(String, Style)>,
    row: Option<Row>,
    /// A small curve before the right-hand text: a profile's, by name.
    thumb: Option<(String, Color)>,
}

impl Line {
    fn text(spans: Vec<(String, Style)>) -> Self {
        Self {
            spans,
            right: Vec::new(),
            row: None,
            thumb: None,
        }
    }

    fn row(row: Row, spans: Vec<(String, Style)>) -> Self {
        Self {
            row: Some(row),
            ..Self::text(spans)
        }
    }

    fn right(mut self, right: Vec<(String, Style)>) -> Self {
        self.right = right;
        self
    }
}

/// A choice from a list, over the page.
struct Choice {
    title: String,
    note: Option<String>,
    items: Vec<Item>,
    cursor: usize,
}

struct Item {
    label: String,
    detail: String,
    chosen: bool,
    pick: Pick,
}

impl Item {
    fn new(label: impl Into<String>, pick: Pick) -> Self {
        Self {
            label: label.into(),
            detail: String::new(),
            chosen: false,
            pick,
        }
    }

    fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    fn chosen(mut self, chosen: bool) -> Self {
        self.chosen = chosen;
        self
    }
}

/// What choosing an item does.
#[derive(Debug, Clone, PartialEq)]
enum Pick {
    /// `None` is the output in use.
    Device(Option<String>),
    Preset(Option<String>),
    /// A row that says what is, and does nothing.
    Nothing,
    Correction(Option<String>),
    AddEq(String),
    NewEq,
    Import(Stage),
    Role(RoleAsk, Option<DspRole>),
    Starter(String, Starter),
    Kind(String, usize, EqFilterKind),
    Target(String, Option<String>),
    MadeAgainst(String, Option<String>),
    SetRole(String, DspRole),
    Member(String, String),
    Keep(String, u16),
    Delete(String),
    Cancel,
}

/// A name or a path to type, over the page.
struct Prompt {
    title: String,
    note: String,
    text: String,
    ask: Ask,
}

#[derive(Debug, Clone, PartialEq)]
enum Ask {
    NewEq,
    Rename(String),
    Copy(String),
    SavePreset,
    Import(Option<Stage>),
    Rate(PendingImport),
}

/// A band's figure being typed.
struct Typing {
    index: usize,
    field: Field,
    text: String,
}

/// Where a click on the page lands.
#[derive(Debug, Clone, PartialEq)]
enum Hit {
    Line(usize),
    Field(usize, usize, Field),
    Item(usize),
    Back,
    /// Over a choice, off its items.
    Inside,
    Outside,
}

/// What the page asks of the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Leave the page.
    Leave,
    Say(String),
}

pub struct EqPage {
    worker: Worker,
    pub snap: Option<Box<Snapshot>>,
    /// What the output in use plays, for Now Playing.
    pub playing: Option<Playing>,
    /// What Now Playing last asked to be drawn, so it asks again only when
    /// that changes.
    pub playing_key: Option<(Option<String>, Option<String>, u32)>,
    picked: Option<String>,
    renderer: Option<(String, String)>,
    views: Vec<View>,
    cursor: usize,
    scroll: usize,
    field: Field,
    typing: Option<Typing>,
    prompt: Option<Prompt>,
    choice: Option<Choice>,
    /// The last thing a change said, and whether it was a refusal.
    message: Option<(String, bool)>,
    seq: u64,
    /// A profile's filters as edited here, ahead of the read that will
    /// show them, and the change that made them.
    local: Option<(String, Vec<DspFilter>, u64)>,
    /// For a profile with bands for each channel, the one being edited.
    channel: u16,
    /// What `B` put away, by device: the preset it is kept as.
    bypassed: Vec<(String, String)>,
    hits: Vec<(Rect, Hit)>,
    /// What the lines of the last frame were, by line index.
    rows: Vec<Option<Row>>,
}

impl EqPage {
    pub fn new(player: crossbeam_channel::Sender<PlayerCommand>) -> Self {
        let page = Self {
            worker: Worker::spawn(player),
            snap: None,
            playing: None,
            playing_key: None,
            picked: None,
            renderer: None,
            views: vec![View::Chain],
            cursor: 0,
            scroll: 0,
            field: Field::Gain,
            typing: None,
            prompt: None,
            choice: None,
            message: None,
            seq: 0,
            local: None,
            channel: 0,
            bypassed: Vec::new(),
            hits: Vec::new(),
            rows: Vec::new(),
        };
        page.request();
        page
    }

    /// The page opened: read it afresh, at the chain.
    pub fn open(&mut self, renderer: Option<(String, String)>) {
        self.renderer = renderer;
        self.views.truncate(1);
        self.cursor = 0;
        self.scroll = 0;
        self.typing = None;
        self.prompt = None;
        self.choice = None;
        self.message = None;
        self.request();
    }

    /// The output in use changed, or the profiles did elsewhere: read again.
    pub fn follow(&mut self, renderer: Option<(String, String)>) {
        self.renderer = renderer;
        self.request();
    }

    fn view(&self) -> &View {
        self.views.last().unwrap_or(&View::Chain)
    }

    fn request(&self) {
        let profile = match self.view() {
            View::Profile(name) => Some(name.clone()),
            _ => None,
        };
        self.worker.send(Job::View(ViewReq {
            device: self.picked.clone(),
            renderer: self.renderer.clone(),
            profile,
        }));
    }

    /// Ask for what the output in use plays, for Now Playing.
    pub fn request_playing(&mut self, renderer: Option<String>, rate: u32) {
        self.worker.send(Job::Playing(renderer, rate));
    }

    /// Takes what the worker has finished. Cheap when it has nothing.
    pub fn poll(&mut self) -> Vec<Outcome> {
        let mut out = Vec::new();
        while let Ok(done) = self.worker.rx.try_recv() {
            match done {
                Done::Snapshot(s) => {
                    if self.local.as_ref().is_some_and(|(_, _, seq)| s.seq >= *seq) {
                        self.local = None;
                    }
                    // A profile renamed or deleted elsewhere: back to the chain.
                    if let View::Profile(name) = self.view()
                        && s.seq >= self.seq
                        && s.asked.as_ref() == Some(name)
                        && s.profile.is_none()
                    {
                        self.views.retain(|v| !matches!(v, View::Profile(_)));
                        self.cursor = 0;
                        self.request();
                    }
                    self.snap = Some(s);
                }
                Done::Reply { seq, result } => match result {
                    Ok(Reply::Said(said)) => {
                        self.message = Some((said.clone(), false));
                        out.push(Outcome::Say(said));
                    }
                    Ok(Reply::Quiet) => {}
                    Ok(Reply::Ask(ask, said)) => {
                        self.message = said.map(|s| (s, false));
                        self.ask_role(ask);
                    }
                    Ok(Reply::NeedsRate(pending)) => {
                        self.prompt = Some(Prompt {
                            title: "sample rate".into(),
                            note: "these coefficients do not say what rate they were designed at; type it, in Hz".into(),
                            text: "48000".into(),
                            ask: Ask::Rate(pending),
                        });
                    }
                    Ok(Reply::Open(name)) => self.open_profile(name),
                    Err(e) => {
                        // A refused edit: what is saved is what shows.
                        if self.local.as_ref().is_some_and(|(_, _, s)| *s <= seq) {
                            self.local = None;
                        }
                        self.message = Some((e.clone(), true));
                        out.push(Outcome::Say(e));
                    }
                },
                Done::Playing(p) => self.playing = p,
            }
        }
        out
    }

    /// Whether the page takes every key: a field or a choice is open.
    pub fn typing(&self) -> bool {
        self.typing.is_some() || self.prompt.is_some() || self.choice.is_some()
    }

    /// What the device shown is set to, in a line, for Settings.
    pub fn summary(&self) -> Option<String> {
        let s = self.snap.as_ref()?;
        let device = s.device.as_deref()?;
        let o = &s.overview;
        let set = match &o.preset {
            Some((p, true)) => format!("{p}, edited"),
            Some((p, false)) => p.clone(),
            None if o.active.is_none() && o.chain.is_empty() => "flat".into(),
            None => "unsaved".into(),
        };
        Some(format!("{} \u{b7} {set}", s.label(device)))
    }

    fn act(
        &mut self,
        key: Option<(String, usize)>,
        run: impl FnOnce() -> Result<Reply, String> + Send + 'static,
    ) {
        self.seq += 1;
        self.worker.send(Job::Act(Act {
            seq: self.seq,
            key,
            run: Box::new(run),
        }));
    }

    fn device(&self) -> Option<String> {
        self.snap.as_ref().and_then(|s| s.device.clone())
    }

    fn label(&self, device: &str) -> String {
        self.snap
            .as_ref()
            .map_or_else(|| device.to_owned(), |s| s.label(device))
    }

    // --- Changes ----------------------------------------------------------

    fn assign(&mut self, name: Option<String>) {
        let Some(device) = self.device() else { return };
        self.act(None, move || {
            profiles::assign(name.as_deref(), &device)?;
            Ok(Reply::Quiet)
        });
    }

    fn set_tunings(&mut self, chain: Vec<(String, bool)>) {
        let Some(device) = self.device() else { return };
        self.act(None, move || {
            profiles::set_tunings(&device, &chain)?;
            Ok(Reply::Quiet)
        });
    }

    fn chain(&self) -> Vec<(String, bool)> {
        self.snap
            .as_ref()
            .map(|s| s.overview.chain.clone())
            .unwrap_or_default()
    }

    fn apply_preset(&mut self, device: String, name: Option<String>) {
        let label = self.label(&device);
        self.act(None, move || {
            profiles::apply_preset(&device, name.as_deref())?;
            Ok(Reply::Said(match name {
                Some(n) => format!("{label}: {n}"),
                None => format!("{label} is flat: it plays untouched"),
            }))
        });
    }

    /// Plays the output in use from the preset `name`, or flat.
    pub fn use_preset(&mut self, name: Option<String>) {
        if let Some(device) = self.snap.as_ref().and_then(|s| s.in_use.clone()) {
            self.apply_preset(device, name);
        }
    }

    /// Bypasses the output in use: flat, what it played kept as a preset to
    /// go back to, which `B` again puts back.
    pub fn bypass(&mut self) -> Option<String> {
        let Some(s) = self.snap.as_ref() else {
            return Some("the eq is still being read".into());
        };
        let Some(device) = s.in_use.clone() else {
            return Some("no output device to set an eq on".into());
        };
        let label = s.label(&device);
        let flat = if s.device.as_ref() == Some(&device) {
            s.overview.active.is_none() && s.overview.chain.is_empty()
        } else {
            s.in_use_flat
        };
        let presets: Vec<String> = s
            .overview
            .profiles
            .iter()
            .filter(|p| p.preset)
            .map(|p| p.name.clone())
            .collect();
        if flat {
            let kept = self
                .bypassed
                .iter()
                .find(|(d, _)| *d == device)
                .map(|(_, p)| p.clone())
                .or_else(|| presets.iter().find(|p| **p == label).cloned());
            let Some(kept) = kept.filter(|k| presets.contains(k)) else {
                return Some(format!(
                    "{label} is flat and nothing was bypassed; choose a preset with E"
                ));
            };
            self.apply_preset(device, Some(kept));
            return None;
        }
        let preset = match &s.overview.preset {
            Some((p, false)) if s.device.as_ref() == Some(&device) => Some(p.clone()),
            _ if s.device.as_ref() != Some(&device) => {
                s.in_use_preset.clone().filter(|(_, e)| !e).map(|(p, _)| p)
            }
            _ => None,
        };
        let kept = preset
            .clone()
            .unwrap_or_else(|| profiles::free_name(&label));
        self.bypassed.retain(|(d, _)| *d != device);
        self.bypassed.push((device.clone(), kept.clone()));
        self.act(None, move || {
            if preset.is_none() {
                profiles::save_preset(&device, &kept)?;
            }
            profiles::apply_preset(&device, None)?;
            Ok(Reply::Said(format!(
                "{label} bypassed, kept as the preset {kept}; B puts it back"
            )))
        });
        None
    }

    fn save_preset(&mut self, name: String) {
        let Some(device) = self.device() else { return };
        self.act(None, move || {
            let name = profiles::save_preset(&device, &name)?;
            Ok(Reply::Said(format!("saved the preset {name}")))
        });
    }

    fn set_filters_only(&mut self, only: bool) {
        let Some(device) = self.device() else { return };
        self.act(None, move || {
            profiles::set_filters_only(&device, only)?;
            Ok(Reply::Quiet)
        });
    }

    fn create_eq(&mut self, name: String, starter: Starter) {
        let Some(device) = self.device() else { return };
        let mut chain = self.chain();
        self.act(None, move || {
            profiles::create_eq(&name, starter)?;
            chain.push((name.clone(), true));
            profiles::set_tunings(&device, &chain)?;
            Ok(Reply::Open(name))
        });
    }

    fn import(&mut self, paths: Vec<PathBuf>, rate: Option<u32>, into: Option<Stage>) {
        let into = into.and_then(|stage| Some((self.device()?, stage)));
        self.act(None, move || {
            match profiles::import_files(&paths, None, rate) {
                Ok(summary) => {
                    let mut said: Vec<String> = Vec::new();
                    if let Some(group) = &summary.group {
                        said.push(format!(
                            "imported {} presets as the group {group}",
                            summary.imported.len()
                        ));
                    } else if paths.len() > 1
                        && let Some(one) = summary.imported.first()
                    {
                        said.push(format!("combined {} files into {one}", paths.len()));
                    } else if let Some(one) = summary.imported.first() {
                        said.push(format!("imported {one}"));
                    }
                    for (file, why) in &summary.refused {
                        said.push(format!("{file} refused: {why}"));
                    }
                    said.extend(summary.notes.iter().cloned());
                    let said = (!said.is_empty()).then(|| said.join(". "));
                    if summary.imported.is_empty() {
                        return Err(said.unwrap_or_else(|| "nothing was imported".into()));
                    }
                    Ok(Reply::Ask(
                        RoleAsk {
                            profiles: summary
                                .group
                                .clone()
                                .map_or_else(|| summary.imported.clone(), |g| vec![g]),
                            names: summary.imported,
                            into,
                        },
                        said,
                    ))
                }
                Err(koan_core::audio::dsp::import::ImportError::NeedsRate(_)) => {
                    Ok(Reply::NeedsRate(PendingImport {
                        paths,
                        name: None,
                        into,
                    }))
                }
                Err(e) => Err(e.to_string()),
            }
        });
    }

    /// The import's question answered: what its profiles are for, `None`
    /// leaving them tunings; one started from a stage goes into it.
    fn answer(&mut self, ask: RoleAsk, role: Option<DspRole>) {
        self.act(None, move || {
            if let Some(role) = role {
                for name in &ask.names {
                    profiles::set_role(name, role)?;
                }
            }
            let Some((device, stage)) = ask.into else {
                return Ok(Reply::Quiet);
            };
            if role.is_some_and(DspRole::corrects) {
                profiles::assign(ask.profiles.first().map(String::as_str), &device)?;
            } else if stage == Stage::Eq {
                let mut chain = profiles::tunings_for(&device);
                for name in ask.profiles {
                    if !chain.iter().any(|(n, _)| *n == name) {
                        chain.push((name, true));
                    }
                }
                profiles::set_tunings(&device, &chain)?;
            }
            Ok(Reply::Quiet)
        });
    }

    fn rename(&mut self, old: String, new: String) {
        let new = new.trim().to_owned();
        if let Some(View::Profile(name)) = self.views.last_mut()
            && *name == old
        {
            *name = new.clone();
        }
        self.act(None, move || {
            profiles::rename(&old, &new)?;
            Ok(Reply::Said(format!("renamed {old} to {new}")))
        });
        self.request();
    }

    fn duplicate(&mut self, name: String, new: String) {
        let new = new.trim().to_owned();
        self.act(None, move || {
            let made = profiles::duplicate(&name, (!new.is_empty()).then_some(new.as_str()))?;
            Ok(Reply::Said(format!("saved {made}, a copy of {name}")))
        });
    }

    fn delete(&mut self, name: String) {
        if matches!(self.view(), View::Profile(n) if *n == name) {
            self.views.pop();
            self.cursor = 0;
        }
        self.act(None, move || {
            profiles::delete(&name)?;
            Ok(Reply::Said(format!("deleted {name}")))
        });
        self.request();
    }

    fn simple(&mut self, run: impl FnOnce() -> Result<(), String> + Send + 'static) {
        self.act(None, move || run().map(|()| Reply::Quiet));
    }

    /// Band `index` of `name` set to `band`: shown at once, saved behind.
    fn set_band(
        &mut self,
        name: &str,
        index: usize,
        kind: EqFilterKind,
        freq: f64,
        gain: f64,
        q: f64,
    ) {
        let mut filters = self.filters().to_vec();
        if let Some(DspFilter::Band(b)) = filters.get_mut(index) {
            (b.kind, b.freq, b.gain_db, b.q) = (kind, freq, gain, q);
        }
        let owned = name.to_owned();
        self.act(Some((owned.clone(), index)), move || {
            profiles::set_band(&owned, index, kind.name(), freq, gain, q)?;
            Ok(Reply::Quiet)
        });
        self.local = Some((name.to_owned(), filters, self.seq));
    }

    // --- Reading ----------------------------------------------------------

    /// The profile open, with its filters as edited here.
    fn filters(&self) -> &[DspFilter] {
        if let (View::Profile(name), Some((n, f, _))) = (self.view(), &self.local)
            && name == n
        {
            return f;
        }
        self.snap
            .as_ref()
            .and_then(|s| s.profile.as_ref())
            .map_or(&[], |p| p.detail.filters.as_slice())
    }

    fn profile(&self) -> Option<&super::eq_worker::ProfileView> {
        let View::Profile(name) = self.view() else {
            return None;
        };
        self.snap
            .as_ref()?
            .profile
            .as_ref()
            .filter(|p| &p.detail.name == name)
    }

    /// The bands shown: every filter, or a stereo EQ's on the channel edited.
    fn shown_filters(&self) -> Vec<usize> {
        let stereo = self
            .profile()
            .and_then(|p| p.detail.channels)
            .is_some_and(|c| c.stereo);
        self.filters()
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                !stereo || f.channels().is_empty() || f.channels().contains(&self.channel)
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn editable(&self) -> bool {
        self.profile().is_some_and(|p| !p.detail.read_only)
    }

    // --- Keys -------------------------------------------------------------

    /// `None`: the page did not take the key.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Option<Outcome>> {
        if self.prompt.is_some() {
            self.prompt_key(key);
            return Some(None);
        }
        if self.choice.is_some() {
            self.choice_key(key);
            return Some(None);
        }
        if self.typing.is_some() {
            self.typing_key(key);
            return Some(None);
        }
        let rows = self.row_list();
        let n = rows.len();
        self.cursor = self.cursor.min(n.saturating_sub(1));
        let row = rows.get(self.cursor).cloned();
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h')
                if !matches!(row, Some(Row::Band(_)) | Some(Row::Device))
                    || key.code == KeyCode::Esc =>
            {
                if self.views.len() > 1 {
                    self.back();
                    return Some(None);
                }
                return if key.code == KeyCode::Esc {
                    Some(Some(Outcome::Leave))
                } else {
                    None
                };
            }
            KeyCode::Up if shift => self.move_eq(row.as_ref(), -1),
            KeyCode::Down if shift => self.move_eq(row.as_ref(), 1),
            KeyCode::Char('K') => self.move_eq(row.as_ref(), -1),
            KeyCode::Char('J') => self.move_eq(row.as_ref(), 1),
            KeyCode::Up | KeyCode::Char('k') => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                self.cursor = (self.cursor + 1).min(n.saturating_sub(1));
            }
            KeyCode::Home | KeyCode::Char('g') => self.cursor = 0,
            KeyCode::End | KeyCode::Char('G') => self.cursor = n.saturating_sub(1),
            KeyCode::PageUp => self.cursor = self.cursor.saturating_sub(10),
            KeyCode::PageDown => self.cursor = (self.cursor + 10).min(n.saturating_sub(1)),
            KeyCode::Enter => {
                if let Some(row) = row {
                    self.activate(row);
                }
            }
            KeyCode::Char(' ') => match row {
                Some(Row::Eq(i)) => self.toggle_eq(i),
                Some(Row::Correct) => self.activate(Row::Correct),
                Some(row) => self.activate(row),
                None => {}
            },
            KeyCode::Char('x') | KeyCode::Delete | KeyCode::Backspace => match row {
                Some(Row::Eq(i)) => {
                    let mut chain = self.chain();
                    if i < chain.len() {
                        let gone = chain.remove(i).0;
                        self.set_tunings(chain);
                        self.message = Some((format!("took {gone} out of the filters"), false));
                    }
                }
                Some(Row::Correction) => self.assign(None),
                Some(Row::Band(i)) if self.editable() => {
                    if let View::Profile(name) = self.view().clone() {
                        let mut filters = self.filters().to_vec();
                        if i < filters.len() {
                            filters.remove(i);
                        }
                        self.act(None, move || {
                            profiles::remove_filter(&name, i)?;
                            Ok(Reply::Quiet)
                        });
                        self.local = Some((self.name(), filters, self.seq));
                    }
                }
                _ => return None,
            },
            KeyCode::Char('o') => match row {
                Some(Row::Correction) => {
                    if let Some(c) = self.snap.as_ref().and_then(|s| s.overview.active.clone()) {
                        self.open_profile(c);
                    }
                }
                Some(Row::Eq(i)) => {
                    if let Some((name, _)) = self.chain().get(i).cloned() {
                        self.open_profile(name);
                    }
                }
                Some(Row::Profile(name)) => self.open_profile(name),
                _ => return None,
            },
            KeyCode::Char('m') if *self.view() == View::Chain => self.go(View::Manage),
            KeyCode::Char('n') if !matches!(self.view(), View::Profile(_)) => self.ask_new_eq(),
            KeyCode::Char('i') if !matches!(self.view(), View::Profile(_)) => {
                let stage = (*self.view() == View::Chain).then_some(Stage::Eq);
                self.ask_import(stage);
            }
            KeyCode::Char('p') if *self.view() == View::Chain => self.choose_preset(),
            KeyCode::Char('s') if *self.view() == View::Chain => self.ask_save_preset(),
            KeyCode::Char('d') if *self.view() == View::Chain => self.cycle_device(1),
            KeyCode::Char('a') if self.editable() => self.add_band(),
            KeyCode::Char('r') => {
                let name = self.target_name(row.as_ref())?;
                self.ask_rename(name);
            }
            KeyCode::Char('c') => {
                let name = self.target_name(row.as_ref())?;
                self.ask_copy(name);
            }
            KeyCode::Char('D') => {
                let name = self.target_name(row.as_ref())?;
                self.confirm_delete(name);
            }
            KeyCode::Char('R') => {
                let name = self.target_name(row.as_ref())?;
                self.simple(move || profiles::revert(&name));
            }
            KeyCode::Char('u') => match row {
                Some(Row::Profile(name)) => self.use_profile(&name),
                _ if matches!(self.view(), View::Profile(_)) => self.activate(Row::Use),
                _ => return None,
            },
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Left | KeyCode::Char('h') => {
                let step = if matches!(key.code, KeyCode::Right | KeyCode::Char('l')) {
                    1
                } else {
                    -1
                };
                match row {
                    Some(Row::Band(_)) => self.move_field(step),
                    Some(Row::Device) => self.cycle_device(step),
                    _ => return None,
                }
            }
            KeyCode::Tab => match row {
                Some(Row::Band(_)) => self.move_field(1),
                _ => return None,
            },
            KeyCode::BackTab => match row {
                Some(Row::Band(_)) => self.move_field(-1),
                _ => return None,
            },
            KeyCode::Char(c @ ('+' | '=' | '-' | '_' | ']' | '[')) => match row {
                Some(Row::Band(i)) => {
                    let up = matches!(c, '+' | '=' | ']');
                    let coarse = matches!(c, ']' | '[');
                    self.nudge(i, up, coarse);
                }
                _ => return None,
            },
            KeyCode::Char(c) if c.is_ascii_digit() || c == '.' => match row {
                Some(Row::Band(i)) if self.editable() && self.field != Field::Kind => {
                    if self.band_editable(i) {
                        self.typing = Some(Typing {
                            index: i,
                            field: self.field,
                            text: c.to_string(),
                        });
                    }
                }
                _ => return None,
            },
            _ => return None,
        }
        Some(None)
    }

    fn name(&self) -> String {
        match self.view() {
            View::Profile(n) => n.clone(),
            _ => String::new(),
        }
    }

    /// The profile a rename, copy or delete would act on from `row`.
    fn target_name(&self, row: Option<&Row>) -> Option<String> {
        match (self.view(), row) {
            (View::Profile(name), _) => Some(name.clone()),
            (_, Some(Row::Profile(name))) => Some(name.clone()),
            (View::Chain, Some(Row::Eq(i))) => self.chain().get(*i).map(|(n, _)| n.clone()),
            (View::Chain, Some(Row::Correction)) => {
                self.snap.as_ref().and_then(|s| s.overview.active.clone())
            }
            _ => None,
        }
    }

    fn go(&mut self, view: View) {
        self.views.push(view);
        self.cursor = 0;
        self.scroll = 0;
        self.request();
    }

    fn back(&mut self) {
        let left = self.views.pop();
        self.typing = None;
        self.cursor = 0;
        self.scroll = 0;
        // Back on the chain, on the stage that was open.
        if let (Some(View::Profile(name)), View::Chain) = (&left, self.view()) {
            let rows = self.row_list();
            let at = self.chain().iter().position(|(n, _)| n == name);
            let to = match at {
                Some(i) => Row::Eq(i),
                None => Row::Correction,
            };
            self.cursor = rows.iter().position(|r| *r == to).unwrap_or(0);
        }
        self.request();
    }

    fn open_profile(&mut self, name: String) {
        if matches!(self.view(), View::Profile(n) if *n == name) {
            return;
        }
        self.channel = 0;
        self.field = Field::Gain;
        self.go(View::Profile(name));
    }

    fn toggle_eq(&mut self, i: usize) {
        let mut chain = self.chain();
        if let Some(entry) = chain.get_mut(i) {
            entry.1 = !entry.1;
            self.set_tunings(chain);
        }
    }

    fn move_eq(&mut self, row: Option<&Row>, step: isize) {
        let Some(Row::Eq(i)) = row else { return };
        let mut chain = self.chain();
        let j = *i as isize + step;
        if j < 0 || j as usize >= chain.len() {
            return;
        }
        chain.swap(*i, j as usize);
        self.set_tunings(chain);
        let rows = self.row_list();
        if let Some(at) = rows.iter().position(|r| *r == Row::Eq(j as usize)) {
            self.cursor = at;
        }
    }

    fn cycle_device(&mut self, step: isize) {
        let Some(s) = self.snap.as_ref() else { return };
        let n = s.devices.len();
        if n < 2 {
            return;
        }
        let at = s
            .device
            .as_ref()
            .and_then(|d| s.devices.iter().position(|x| x == d))
            .unwrap_or(0);
        let next = s.devices[(at as isize + step).rem_euclid(n as isize) as usize].clone();
        self.picked = (s.in_use.as_ref() != Some(&next)).then_some(next);
        self.request();
    }

    fn move_field(&mut self, step: isize) {
        let at = FIELDS.iter().position(|f| *f == self.field).unwrap_or(0);
        self.field = FIELDS[(at as isize + step).clamp(0, 3) as usize];
    }

    fn band_editable(&self, i: usize) -> bool {
        matches!(self.filters().get(i), Some(DspFilter::Band(b)) if KINDS.iter().any(|(k, _)| *k == b.kind))
    }

    fn nudge(&mut self, i: usize, up: bool, coarse: bool) {
        if !self.editable() || !self.band_editable(i) {
            return;
        }
        let Some(DspFilter::Band(b)) = self.filters().get(i).cloned() else {
            return;
        };
        let sign = if up { 1.0 } else { -1.0 };
        let (mut kind, mut freq, mut gain, mut q) = (b.kind, b.freq, b.gain_db, b.q);
        match self.field {
            Field::Kind => {
                let at = KINDS.iter().position(|(k, _)| *k == kind).unwrap_or(0);
                kind = KINDS
                    [(at as isize + sign as isize).rem_euclid(KINDS.len() as isize) as usize]
                    .0;
            }
            Field::Freq => {
                let octaves = if coarse { 1.0 / 3.0 } else { 1.0 / 12.0 };
                freq = (freq * 2f64.powf(sign * octaves)).clamp(10.0, 22_000.0);
                freq = if freq >= 1000.0 {
                    freq.round()
                } else {
                    (freq * 10.0).round() / 10.0
                };
            }
            Field::Gain => {
                let step = if coarse { 1.0 } else { 0.1 };
                gain = (((gain + sign * step) * 10.0).round() / 10.0).clamp(-30.0, 30.0);
            }
            Field::Q => {
                let factor = 2f64.powf(sign * if coarse { 0.5 } else { 1.0 / 12.0 });
                q = ((q * factor * 100.0).round() / 100.0).clamp(0.1, 20.0);
            }
        }
        let name = self.name();
        self.set_band(&name, i, kind, freq, gain, q);
    }

    fn add_band(&mut self) {
        let name = self.name();
        let stereo = self
            .profile()
            .and_then(|p| p.detail.channels)
            .is_some_and(|c| c.stereo);
        let channel = stereo.then_some(self.channel);
        self.act(None, move || {
            profiles::add_band(&name, channel)?;
            Ok(Reply::Quiet)
        });
        // The new band lands where "add a band" is, which moves down one.
        if let Some(at) = self.row_list().iter().position(|r| *r == Row::AddBand) {
            self.cursor = at;
        }
        self.field = Field::Freq;
    }

    fn use_profile(&mut self, name: &str) {
        let Some(s) = self.snap.as_ref() else { return };
        let Some(p) = s.overview.profiles.iter().find(|p| p.name == name) else {
            return;
        };
        let Some(device) = s.device.clone() else {
            return;
        };
        if p.preset {
            self.apply_preset(device, Some(name.to_owned()));
        } else if p.role == DspRole::Tuning && p.rates.is_empty() {
            let label = s.label(&device);
            let mut chain = self.chain();
            if !chain.iter().any(|(n, _)| n == name) {
                chain.push((name.to_owned(), true));
                self.set_tunings(chain);
                self.message = Some((format!("added {name} to {label}'s filters"), false));
            }
        } else {
            self.assign(Some(name.to_owned()));
        }
    }

    fn activate(&mut self, row: Row) {
        match row {
            Row::Device => self.choose_device(),
            Row::Preset => self.choose_preset(),
            Row::Correct => {
                let only = self.snap.as_ref().is_some_and(|s| s.overview.filters_only);
                self.set_filters_only(!only);
            }
            Row::SaveOver(p) => self.save_preset(p),
            Row::Revert(p) => {
                if let Some(d) = self.device() {
                    self.apply_preset(d, Some(p));
                }
            }
            Row::SaveNew => self.ask_save_preset(),
            Row::Correction => self.choose_stage(Stage::Correction),
            Row::Eq(i) => {
                if let Some((name, _)) = self.chain().get(i).cloned() {
                    self.open_profile(name);
                }
            }
            Row::AddEq => self.choose_stage(Stage::Eq),
            Row::Manage => self.go(View::Manage),
            Row::Role => self.choose_role(),
            Row::MadeAgainst => self.choose_made_against(),
            Row::CorrectedTo => self.choose_target(),
            Row::Member => self.choose_member(),
            Row::Channels => {
                let Some(layout) = self.profile().and_then(|p| p.detail.channels) else {
                    return;
                };
                let name = self.name();
                if layout.stereo && layout.differ {
                    self.choice = Some(Choice {
                        title: "keep which channel?".into(),
                        note: Some("the two channels have different bands; one list plays on both, and the other's bands are dropped".into()),
                        items: vec![
                            Item::new("keep left", Pick::Keep(name.clone(), 0)),
                            Item::new("keep right", Pick::Keep(name, 1)),
                            Item::new("cancel", Pick::Cancel),
                        ],
                        cursor: 0,
                    });
                } else {
                    let stereo = !layout.stereo;
                    self.channel = 0;
                    self.simple(move || profiles::set_stereo(&name, stereo, 0));
                }
            }
            Row::Editing => self.channel = 1 - self.channel.min(1),
            Row::Band(i) => {
                if !self.editable() || !self.band_editable(i) {
                    return;
                }
                if self.field == Field::Kind {
                    self.choose_kind(i);
                } else {
                    let text = self.figure(i, self.field);
                    self.typing = Some(Typing {
                        index: i,
                        field: self.field,
                        text,
                    });
                }
            }
            Row::AddBand => self.add_band(),
            Row::Use => {
                let Some(p) = self.profile() else { return };
                let Some(in_use) = self.snap.as_ref().and_then(|s| s.in_use.clone()) else {
                    return;
                };
                let name = p.detail.name.clone();
                if p.detail.devices.contains(&in_use) {
                    self.simple(move || profiles::assign(None, &in_use));
                } else if p.detail.role == DspRole::Tuning {
                    let chain = profiles_chain_with(&self.chain(), &name);
                    self.act(None, move || {
                        profiles::set_tunings(&in_use, &chain)?;
                        Ok(Reply::Quiet)
                    });
                } else {
                    self.simple(move || profiles::assign(Some(&name), &in_use));
                }
            }
            Row::Sync => {
                let Some(p) = self.profile() else { return };
                let name = p.detail.name.clone();
                let to = if p.detail.everywhere {
                    DspScope::Device
                } else {
                    DspScope::Everywhere
                };
                self.simple(move || profiles::set_scope(&name, to));
            }
            Row::Rename => self.ask_rename(self.name()),
            Row::Copy => self.ask_copy(self.name()),
            Row::Reset => {
                let name = self.name();
                self.simple(move || profiles::revert(&name));
            }
            Row::Delete => self.confirm_delete(self.name()),
            Row::Profile(name) => self.open_profile(name),
            Row::Import => self.ask_import(None),
            Row::New => self.ask_new_eq(),
        }
    }

    // --- Choices ----------------------------------------------------------

    fn choose_device(&mut self) {
        let Some(s) = self.snap.as_ref() else { return };
        let items = s
            .devices
            .iter()
            .map(|d| {
                let label = if s.in_use.as_ref() == Some(d) {
                    format!("{} (in use)", s.label(d))
                } else {
                    s.label(d)
                };
                let pick = if s.in_use.as_ref() == Some(d) {
                    Pick::Device(None)
                } else {
                    Pick::Device(Some(d.clone()))
                };
                Item::new(label, pick).chosen(s.device.as_ref() == Some(d))
            })
            .collect();
        self.open_choice("device", None, items);
    }

    fn choose_preset(&mut self) {
        let Some(s) = self.snap.as_ref() else { return };
        let o = &s.overview;
        let flat = o.active.is_none() && o.chain.is_empty();
        let mut items = vec![
            Item::new("flat", Pick::Preset(None))
                .detail("plays untouched, bit for bit")
                .chosen(o.preset.is_none() && flat),
        ];
        if o.preset.is_none() && !flat {
            items.push(Item::new("unsaved", Pick::Nothing).chosen(true));
        }
        for p in o.profiles.iter().filter(|p| p.preset) {
            if let Some((current, true)) = &o.preset
                && *current == p.name
            {
                items.push(Item::new(format!("{} (edited)", p.name), Pick::Nothing).chosen(true));
            }
            items.push(
                Item::new(p.name.clone(), Pick::Preset(Some(p.name.clone())))
                    .detail(describe(p))
                    .chosen(o.preset.as_ref().is_some_and(|(c, e)| *c == p.name && !e)),
            );
        }
        let device = s.device.as_deref().map(|d| s.label(d)).unwrap_or_default();
        self.open_choice("preset", Some(format!("what {device} is set from")), items);
    }

    fn choose_stage(&mut self, stage: Stage) {
        let Some(s) = self.snap.as_ref() else { return };
        let o = &s.overview;
        let mut items = Vec::new();
        match stage {
            Stage::Correction => {
                items.push(
                    Item::new("none", Pick::Correction(None))
                        .detail("no correction: the device as it is")
                        .chosen(o.active.is_none()),
                );
                for p in o
                    .profiles
                    .iter()
                    .filter(|p| !p.preset && p.role != DspRole::Tuning)
                {
                    items.push(
                        Item::new(p.name.clone(), Pick::Correction(Some(p.name.clone())))
                            .detail(describe(p))
                            .chosen(o.active.as_ref() == Some(&p.name)),
                    );
                }
            }
            Stage::Eq => {
                let mut eqs: Vec<&Summary> = o
                    .profiles
                    .iter()
                    .filter(|p| {
                        !p.preset
                            && p.role == DspRole::Tuning
                            && p.rates.is_empty()
                            && !o.chain.iter().any(|(n, _)| *n == p.name)
                    })
                    .collect();
                eqs.sort_by_key(|p| (p.join != Some(Join::Matched), p.name.to_lowercase()));
                for p in eqs {
                    let mut detail = describe(p);
                    if let (Some(made), false) = (&p.made_for, o.filters_only) {
                        let mark = if p.join == Some(Join::Matched) {
                            " \u{2713}"
                        } else {
                            ""
                        };
                        detail = format!("made for {made}{mark} \u{b7} {detail}");
                    }
                    items.push(
                        Item::new(p.name.clone(), Pick::AddEq(p.name.clone())).detail(detail),
                    );
                }
                if items.is_empty() {
                    items.push(
                        Item::new("every EQ is in the filters already", Pick::Nothing)
                            .detail("make a new one or import another below"),
                    );
                }
                items.push(Item::new("+ new EQ\u{2026}", Pick::NewEq));
            }
        }
        items.push(
            Item::new("+ import a file\u{2026}", Pick::Import(stage))
                .detail("a path to a file, a folder or a zip"),
        );
        let title = match stage {
            Stage::Correction => "correction",
            Stage::Eq => "add EQ",
        };
        let note = match stage {
            Stage::Correction => "makes headphones or speakers neutral; the filters go on top",
            Stage::Eq => "goes on the end of the filters, switched on",
        };
        self.open_choice(title, Some(note.into()), items);
    }

    fn choose_role(&mut self) {
        let Some(p) = self.profile() else { return };
        let name = p.detail.name.clone();
        let role = p.detail.role;
        let items = [
            (DspRole::Correction, "makes headphones or speakers neutral"),
            (
                DspRole::Baked,
                "corrects and adds taste in one, as most finished presets do",
            ),
            (
                DspRole::Tuning,
                "taste on top of a correction: more bass, a darker treble",
            ),
        ]
        .into_iter()
        .map(|(r, what)| {
            Item::new(role_label(r), Pick::SetRole(name.clone(), r))
                .detail(what)
                .chosen(r == role)
        })
        .collect();
        self.open_choice("what it's for", None, items);
    }

    fn choose_made_against(&mut self) {
        let Some(p) = self.profile() else { return };
        let name = p.detail.name.clone();
        let current = p.detail.tuned_for.clone();
        let mut items = vec![
            Item::new("unknown", Pick::MadeAgainst(name.clone(), None))
                .detail("nothing converts: a target the EQ includes may play twice")
                .chosen(current.is_none()),
        ];
        items.extend(p.made_against.iter().map(|(id, label)| {
            Item::new(
                label.clone(),
                Pick::MadeAgainst(name.clone(), Some(id.clone())),
            )
            .chosen(current.as_ref() == Some(id))
        }));
        self.open_choice(
            "made against",
            Some("on a device corrected to another target, the difference plays first, so it sounds as it was made".into()),
            items,
        );
    }

    fn choose_target(&mut self) {
        let Some(p) = self.profile() else { return };
        let Some(t) = p.targets.as_ref() else { return };
        let name = p.detail.name.clone();
        let made_for = t.made_for.map(|m| m.id.to_owned());
        let current = t.chosen.clone().or_else(|| made_for.clone());
        let items = t
            .choices
            .iter()
            .map(|c| {
                let label = if made_for.as_ref() == Some(&c.id) {
                    format!("{} (default)", c.name)
                } else {
                    c.name.clone()
                };
                let pick = if made_for.as_ref() == Some(&c.id) {
                    Pick::Target(name.clone(), None)
                } else {
                    Pick::Target(name.clone(), Some(c.id.clone()))
                };
                Item::new(label, pick)
                    .detail(c.does.clone())
                    .chosen(current.as_ref() == Some(&c.id))
            })
            .collect();
        self.open_choice("corrected to", None, items);
    }

    fn choose_member(&mut self) {
        let Some(p) = self.profile() else { return };
        let name = p.detail.name.clone();
        let playing = p
            .detail
            .layers
            .iter()
            .find(|l| l.on)
            .or(p.detail.layers.first())
            .map(|l| l.profile.clone());
        let items = p
            .detail
            .layers
            .iter()
            .map(|l| {
                Item::new(
                    l.profile.clone(),
                    Pick::Member(name.clone(), l.profile.clone()),
                )
                .chosen(playing.as_ref() == Some(&l.profile))
            })
            .collect();
        self.open_choice("playing", Some("one member plays at a time".into()), items);
    }

    fn choose_kind(&mut self, index: usize) {
        let Some(DspFilter::Band(b)) = self.filters().get(index) else {
            return;
        };
        let name = self.name();
        let items = KINDS
            .iter()
            .map(|(k, label)| {
                Item::new(*label, Pick::Kind(name.clone(), index, *k)).chosen(*k == b.kind)
            })
            .collect();
        self.open_choice("band type", None, items);
    }

    fn ask_role(&mut self, ask: RoleAsk) {
        let many = ask.names.len() > 1;
        let title = if many {
            "what are these EQs?"
        } else {
            "what is this EQ?"
        };
        let note = match &ask.into {
            Some((device, Stage::Eq)) => format!(
                "koan can't tell from the file. A tuning is added to {}'s filters; a correction becomes its correction.",
                self.label(device)
            ),
            Some((device, Stage::Correction)) => format!(
                "koan can't tell from the file. A correction becomes {}'s correction.",
                self.label(device)
            ),
            None => "koan can't tell from the file what it does.".into(),
        };
        let items = vec![
            Item::new("correction", Pick::Role(ask.clone(), Some(DspRole::Correction)))
                .detail("makes headphones or speakers neutral: an AutoEQ result, a correction from a measurement"),
            Item::new("correction + tuning", Pick::Role(ask.clone(), Some(DspRole::Baked)))
                .detail("corrects and adds taste in one, as presets named for a sound do"),
            Item::new("tuning", Pick::Role(ask.clone(), Some(DspRole::Tuning)))
                .detail("taste on top of a correction: more bass, a darker treble"),
            Item::new("decide later", Pick::Role(ask, None))
                .detail("kept as a tuning; change it on its page"),
        ];
        self.open_choice(title, Some(note), items);
    }

    fn confirm_delete(&mut self, name: String) {
        let Some(s) = self.snap.as_ref() else { return };
        let p = s.overview.profiles.iter().find(|p| p.name == name);
        let mut note = Vec::new();
        if let Some(p) = p {
            if !p.used_on.is_empty() {
                let on: Vec<String> = p.used_on.iter().map(|d| s.label(d)).collect();
                note.push(format!(
                    "used on {}, which will play without it.",
                    on.join(", ")
                ));
            }
            if !p.held_by.is_empty() {
                note.push(format!("it is taken out of {}.", p.held_by.join(", ")));
            }
        }
        if note.is_empty() {
            note.push("it is not used anywhere.".into());
        }
        let items = vec![
            Item::new("cancel", Pick::Cancel),
            Item::new(format!("delete {name}"), Pick::Delete(name.clone())),
        ];
        self.open_choice(&format!("delete {name}?"), Some(note.join(" ")), items);
    }

    fn open_choice(&mut self, title: &str, note: Option<String>, items: Vec<Item>) {
        let cursor = items.iter().position(|i| i.chosen).unwrap_or(0);
        self.choice = Some(Choice {
            title: title.into(),
            note,
            items,
            cursor,
        });
    }

    fn choice_key(&mut self, key: KeyEvent) {
        let Some(c) = self.choice.as_mut() else {
            return;
        };
        let n = c.items.len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.choice = None,
            KeyCode::Up | KeyCode::Char('k') => c.cursor = (c.cursor + n - 1) % n.max(1),
            KeyCode::Down | KeyCode::Char('j') => c.cursor = (c.cursor + 1) % n.max(1),
            KeyCode::Home | KeyCode::Char('g') => c.cursor = 0,
            KeyCode::End | KeyCode::Char('G') => c.cursor = n.saturating_sub(1),
            KeyCode::Enter | KeyCode::Char(' ') => {
                let i = c.cursor;
                self.pick(i);
            }
            _ => {}
        }
    }

    fn pick(&mut self, i: usize) {
        let Some(c) = self.choice.take() else { return };
        let Some(item) = c.items.into_iter().nth(i) else {
            return;
        };
        match item.pick {
            Pick::Device(d) => {
                self.picked = d;
                self.request();
            }
            Pick::Preset(p) => {
                if let Some(d) = self.device() {
                    self.apply_preset(d, p);
                }
            }
            Pick::Nothing | Pick::Cancel => {}
            Pick::Correction(c) => self.assign(c),
            Pick::AddEq(name) => {
                let mut chain = self.chain();
                chain.push((name, true));
                self.set_tunings(chain);
            }
            Pick::NewEq => self.ask_new_eq(),
            Pick::Import(stage) => self.ask_import(Some(stage)),
            Pick::Role(ask, role) => self.answer(ask, role),
            Pick::Starter(name, starter) => self.create_eq(name, starter),
            Pick::Kind(name, index, kind) => {
                if let Some(DspFilter::Band(b)) = self.filters().get(index).cloned() {
                    self.set_band(&name, index, kind, b.freq, b.gain_db, b.q);
                }
            }
            Pick::Target(name, id) => {
                self.simple(move || profiles::choose_target(&name, id.as_deref()));
            }
            Pick::MadeAgainst(name, id) => {
                self.simple(move || profiles::set_tuned_for(&name, id.as_deref()));
            }
            Pick::SetRole(name, role) => self.simple(move || profiles::set_role(&name, role)),
            Pick::Member(group, member) => {
                self.simple(move || profiles::select(&group, &member));
            }
            Pick::Keep(name, keep) => {
                self.channel = 0;
                self.simple(move || profiles::set_stereo(&name, false, keep));
            }
            Pick::Delete(name) => self.delete(name),
        }
    }

    // --- Prompts ----------------------------------------------------------

    fn ask(&mut self, title: &str, note: &str, text: String, ask: Ask) {
        self.prompt = Some(Prompt {
            title: title.into(),
            note: note.into(),
            text,
            ask,
        });
    }

    fn ask_new_eq(&mut self) {
        self.ask(
            "new EQ",
            "a name; it starts as eight bands across the range and goes on the end of the filters",
            String::new(),
            Ask::NewEq,
        );
    }

    fn ask_rename(&mut self, name: String) {
        self.ask("rename", "a new name", name.clone(), Ask::Rename(name));
    }

    fn ask_copy(&mut self, name: String) {
        self.ask(
            "save as new",
            "a copy as it is now, to change apart from the original",
            format!("{name} copy"),
            Ask::Copy(name),
        );
    }

    fn ask_save_preset(&mut self) {
        let Some(s) = self.snap.as_ref() else { return };
        let o = &s.overview;
        if o.active.is_none() && o.chain.is_empty() {
            self.message = Some(("flat: nothing to save as a preset".into(), true));
            return;
        }
        let device = s.device.as_deref().map(|d| s.label(d)).unwrap_or_default();
        self.ask(
            "save as preset",
            &format!(
                "the correction and filters, to switch {device} back to, or another device to"
            ),
            String::new(),
            Ask::SavePreset,
        );
    }

    fn ask_import(&mut self, into: Option<Stage>) {
        self.ask(
            "import",
            "a path, typed or pasted: AutoEQ and Equalizer APO text, impulse WAVs, Roon zips, Convolver .cfg, CamillaDSP. Several paths, one per line or quoted, become a group",
            String::new(),
            Ask::Import(into),
        );
    }

    /// Pasted text: into the field open, or on the page, files to import.
    pub fn paste(&mut self, text: &str) {
        if let Some(p) = self.prompt.as_mut() {
            let line = if matches!(p.ask, Ask::Import(_)) {
                text.trim_end_matches(['\n', '\r']).to_owned()
            } else {
                text.lines().next().unwrap_or("").to_owned()
            };
            p.text.push_str(&line);
            return;
        }
        if let Some(t) = self.typing.as_mut() {
            t.text.push_str(text.trim());
            return;
        }
        if self.choice.is_some() {
            return;
        }
        let into = (*self.view() == View::Chain).then_some(Stage::Eq);
        self.ask_import(into);
        if let Some(p) = self.prompt.as_mut() {
            p.text = text.trim().to_owned();
        }
    }

    fn prompt_key(&mut self, key: KeyEvent) {
        let Some(p) = self.prompt.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.prompt = None,
            KeyCode::Enter => self.confirm_prompt(),
            KeyCode::Backspace => {
                p.text.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => p.text.clear(),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => p.text.push(c),
            _ => {}
        }
    }

    fn confirm_prompt(&mut self) {
        let Some(p) = self.prompt.take() else { return };
        let text = p.text.trim().to_owned();
        if text.is_empty() {
            self.prompt = Some(p);
            return;
        }
        match p.ask {
            Ask::NewEq => {
                let taken = self
                    .snap
                    .as_ref()
                    .is_some_and(|s| s.overview.profiles.iter().any(|q| q.name == text));
                if taken {
                    self.message = Some((format!("there is already an EQ called {text}"), true));
                    self.prompt = Some(Prompt { text, ..p });
                    return;
                }
                let items = [
                    ("flat", Starter::Flat, "every band at 0 dB"),
                    ("bass boost", Starter::BassBoost, "+4 dB low shelf"),
                    (
                        "warm",
                        Starter::Warm,
                        "a little more bass, a little less treble",
                    ),
                    ("bright", Starter::Bright, "more treble and presence"),
                ]
                .into_iter()
                .map(|(label, s, what)| {
                    Item::new(label, Pick::Starter(text.clone(), s)).detail(what)
                })
                .collect();
                self.open_choice(
                    "start from",
                    Some("a low shelf, six peaks an octave apart from 200 Hz, and a high shelf; every band can be moved, retyped or removed".into()),
                    items,
                );
            }
            Ask::Rename(old) => self.rename(old, text),
            Ask::Copy(name) => self.duplicate(name, text),
            Ask::SavePreset => {
                let taken = self.snap.as_ref().is_some_and(|s| {
                    s.overview
                        .profiles
                        .iter()
                        .any(|q| q.preset && q.name == text)
                });
                if taken {
                    self.message = Some((
                        format!("there is already a preset called {text}; choose another name"),
                        true,
                    ));
                    self.prompt = Some(Prompt { text, ..p });
                    return;
                }
                self.save_preset(text);
            }
            Ask::Import(into) => {
                let paths = parse_paths(&p.text);
                let missing: Vec<String> = paths
                    .iter()
                    .filter(|p| !p.exists())
                    .map(|p| p.display().to_string())
                    .collect();
                if paths.is_empty() || !missing.is_empty() {
                    self.message = Some((format!("no such file: {}", missing.join(", ")), true));
                    self.prompt = Some(Prompt { text: p.text, ..p });
                    return;
                }
                self.import(paths, None, into);
            }
            Ask::Rate(pending) => {
                let Ok(rate) = text.trim_end_matches("Hz").trim().parse::<u32>() else {
                    self.message = Some(("a rate in Hz, such as 48000".into(), true));
                    self.prompt = Some(Prompt {
                        text,
                        ask: Ask::Rate(pending),
                        ..p
                    });
                    return;
                };
                let stage = pending.into.as_ref().map(|(_, s)| *s);
                self.import(pending.paths, Some(rate), stage);
            }
        }
    }

    fn typing_key(&mut self, key: KeyEvent) {
        let Some(t) = self.typing.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.typing = None,
            KeyCode::Enter => self.commit_typing(0),
            KeyCode::Tab | KeyCode::Right => self.commit_typing(1),
            KeyCode::BackTab | KeyCode::Left => self.commit_typing(-1),
            KeyCode::Up | KeyCode::Down => {
                self.commit_typing(0);
                let rows = self.row_list();
                let n = rows.len();
                self.cursor = if key.code == KeyCode::Up {
                    self.cursor.saturating_sub(1)
                } else {
                    (self.cursor + 1).min(n.saturating_sub(1))
                };
            }
            KeyCode::Backspace => {
                t.text.pop();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => t.text.push(c),
            _ => {}
        }
    }

    /// The figure typed, saved; then the field `step` along.
    fn commit_typing(&mut self, step: isize) {
        let Some(t) = self.typing.take() else { return };
        let Some(DspFilter::Band(b)) = self.filters().get(t.index).cloned() else {
            return;
        };
        let value = match t.field {
            Field::Freq => parse_hz(&t.text),
            Field::Gain => parse_number(&t.text, "db"),
            Field::Q => parse_number(&t.text, "q"),
            Field::Kind => None,
        };
        match value {
            Some(v) => {
                let (mut freq, mut gain, mut q) = (b.freq, b.gain_db, b.q);
                match t.field {
                    Field::Freq => freq = v.clamp(10.0, 22_000.0),
                    Field::Gain => gain = v.clamp(-30.0, 30.0),
                    Field::Q => q = v.clamp(0.1, 20.0),
                    Field::Kind => {}
                }
                let name = self.name();
                self.set_band(&name, t.index, b.kind, freq, gain, q);
            }
            None => {
                self.message = Some((format!("{} is not a number", t.text), true));
            }
        }
        if step != 0 {
            let at = FIELDS.iter().position(|f| *f == t.field).unwrap_or(0) as isize + step;
            if (1..=3).contains(&at) {
                self.field = FIELDS[at as usize];
                let text = self.figure(t.index, self.field);
                self.typing = Some(Typing {
                    index: t.index,
                    field: self.field,
                    text,
                });
            } else {
                self.field = FIELDS[at.clamp(0, 3) as usize];
            }
        }
    }

    /// A band's figure as the table shows it.
    fn figure(&self, i: usize, field: Field) -> String {
        let Some(DspFilter::Band(b)) = self.filters().get(i) else {
            return String::new();
        };
        match field {
            Field::Kind => kind_label(b.kind).into(),
            Field::Freq => hz_label(b.freq),
            Field::Gain => format!("{:+.1}", b.gain_db),
            Field::Q => format!("{:.2}", b.q),
        }
    }

    // --- Mouse ------------------------------------------------------------

    /// A click on the page. True when it landed on something of the page's.
    pub fn click(&mut self, x: u16, y: u16) -> Option<Outcome> {
        let hit = self
            .hits
            .iter()
            .rev()
            .find(|(r, _)| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height)
            .map(|(_, h)| h.clone());
        match hit {
            Some(Hit::Item(i)) => {
                if let Some(c) = self.choice.as_mut() {
                    c.cursor = i;
                }
                self.pick(i);
            }
            Some(Hit::Inside) | None => {}
            Some(Hit::Outside) => {
                self.choice = None;
                self.prompt = None;
            }
            Some(Hit::Back) => {
                if self.views.len() > 1 {
                    self.back();
                } else {
                    return Some(Outcome::Leave);
                }
            }
            Some(Hit::Line(line)) => {
                if self.typing.is_some() {
                    self.commit_typing(0);
                }
                if let Some(Some(row)) = self.rows.get(line).cloned() {
                    let rows = self.row_list();
                    let at = rows.iter().position(|r| *r == row);
                    let again = at == Some(self.cursor);
                    if let Some(at) = at {
                        self.cursor = at;
                    }
                    // A click on a row chooses it, as Enter does; on a band,
                    // the first click selects it.
                    if !matches!(row, Row::Band(_)) || again {
                        self.activate(row);
                    }
                }
            }
            Some(Hit::Field(line, index, field)) => {
                if self.typing.is_some() {
                    self.commit_typing(0);
                }
                if let Some(Some(row)) = self.rows.get(line).cloned()
                    && let Some(at) = self.row_list().iter().position(|r| *r == row)
                {
                    let again = at == self.cursor && self.field == field;
                    self.cursor = at;
                    self.field = field;
                    if again {
                        self.activate(Row::Band(index));
                    }
                }
            }
        }
        None
    }

    pub fn scroll_by(&mut self, delta: isize) {
        if let Some(c) = self.choice.as_mut() {
            let n = c.items.len().max(1);
            c.cursor = (c.cursor as isize + delta).rem_euclid(n as isize) as usize;
            return;
        }
        let n = self.row_list().len();
        self.cursor =
            (self.cursor as isize + delta).clamp(0, n.saturating_sub(1) as isize) as usize;
    }

    /// The wheel over a band's figure nudges it.
    pub fn wheel_at(&mut self, x: u16, y: u16, up: bool) -> bool {
        let hit = self
            .hits
            .iter()
            .find(|(r, _)| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height)
            .map(|(_, h)| h.clone());
        if let Some(Hit::Field(line, index, field)) = hit
            && field != Field::Kind
            && self.choice.is_none()
        {
            if let Some(Some(row)) = self.rows.get(line).cloned()
                && let Some(at) = self.row_list().iter().position(|r| *r == row)
            {
                self.cursor = at;
            }
            self.field = field;
            self.nudge(index, up, false);
            return true;
        }
        false
    }

    // --- Rows -------------------------------------------------------------

    /// The rows of the view, in order: what the cursor moves through.
    fn row_list(&self) -> Vec<Row> {
        self.lines(&Theme::default())
            .into_iter()
            .filter_map(|l| l.row)
            .collect()
    }

    fn lines(&self, theme: &Theme) -> Vec<Line> {
        let Some(s) = self.snap.as_deref() else {
            return vec![Line::text(vec![(
                "reading the eq\u{2026}".into(),
                theme.hint_desc,
            )])];
        };
        match self.view() {
            View::Chain => self.chain_lines(s, theme),
            View::Profile(_) => self.profile_lines(s, theme),
            View::Manage => manage_lines(s, theme),
        }
    }

    fn chain_lines(&self, s: &Snapshot, theme: &Theme) -> Vec<Line> {
        let o = &s.overview;
        let muted = theme.hint_desc;
        let ink = theme.track_normal;
        let accent = Style::new().fg(theme.tokens.accent);
        let key = |k: &str| (format!("{k:<10}"), muted);
        let mut out = Vec::new();
        let Some(device) = s.device.as_deref() else {
            out.push(Line::text(vec![(
                "no output device to set an eq on".into(),
                muted,
            )]));
            return out;
        };
        let label = s.label(device);
        let flat = o.active.is_none() && o.chain.is_empty();
        let in_use = s.in_use.as_deref() == Some(device);
        out.push(
            Line::row(
                Row::Device,
                vec![
                    key("device"),
                    (label.clone(), ink.add_modifier(Modifier::BOLD)),
                    (
                        if in_use {
                            "  in use".into()
                        } else {
                            String::new()
                        },
                        muted,
                    ),
                ],
            )
            .right(if s.devices.len() > 1 {
                vec![("\u{2039} \u{203a}".into(), muted)]
            } else {
                Vec::new()
            }),
        );
        let preset = match &o.preset {
            Some((p, true)) => vec![(p.clone(), ink), (" (edited)".into(), muted)],
            Some((p, false)) => vec![(p.clone(), ink)],
            None if flat => vec![("flat".into(), ink), ("  plays untouched".into(), muted)],
            None => vec![("unsaved".into(), ink)],
        };
        out.push(Line::row(
            Row::Preset,
            [vec![key("preset")], preset].concat(),
        ));
        let check = if o.filters_only { "[ ]" } else { "[x]" };
        out.push(Line::row(
            Row::Correct,
            vec![
                key(""),
                (format!("{check} correct for a headphone profile"), ink),
            ],
        ));
        match &o.preset {
            Some((p, true)) => {
                out.push(Line::row(
                    Row::SaveOver(p.clone()),
                    vec![key(""), (format!("save changes to {p}"), accent)],
                ));
                out.push(Line::row(
                    Row::Revert(p.clone()),
                    vec![key(""), (format!("revert to {p}"), accent)],
                ));
                out.push(Line::row(
                    Row::SaveNew,
                    vec![key(""), ("save as new preset\u{2026}".into(), accent)],
                ));
            }
            None if !flat => out.push(Line::row(
                Row::SaveNew,
                vec![key(""), ("save as preset\u{2026}".into(), accent)],
            )),
            _ => {}
        }
        out.push(Line::text(Vec::new()));
        let rule = Style::new().fg(theme.tokens.rule);
        let line_style = if chain_matched(s) { accent } else { rule };
        out.push(Line::text(vec![
            ("\u{266a} ".into(), muted),
            ("music in".into(), muted),
        ]));
        let correction = o
            .profiles
            .iter()
            .find(|p| Some(&p.name) == o.active.as_ref());
        if !o.filters_only {
            let mut l = match correction {
                Some(c) => {
                    let mut detail = vec![describe(c)];
                    if c.role == DspRole::Baked {
                        detail = vec!["already includes a tuning".into()];
                    } else if let Some(aim) = &o.aim {
                        detail.push(format!("to {aim}"));
                    }
                    let detail = detail
                        .into_iter()
                        .filter(|d| !d.is_empty())
                        .collect::<Vec<_>>()
                        .join(" \u{b7} ");
                    let mut l = Line::row(
                        Row::Correction,
                        vec![
                            ("\u{251c}\u{2500} ".into(), line_style),
                            ("correction  ".into(), muted),
                            (c.name.clone(), ink.add_modifier(Modifier::BOLD)),
                            (format!("  {detail}"), muted),
                        ],
                    );
                    l.thumb = Some((c.name.clone(), theme.tokens.accent));
                    l
                }
                None => Line::row(
                    Row::Correction,
                    vec![
                        ("\u{251c}\u{2500} ".into(), line_style),
                        ("correction  ".into(), muted),
                        ("+ add a correction".into(), accent),
                    ],
                ),
            };
            l.right = Vec::new();
            out.push(l);
        }
        out.push(Line::text(vec![
            ("\u{2502}  ".into(), line_style),
            ("filters".into(), muted),
        ]));
        let baked = correction.is_some_and(|c| c.role == DspRole::Baked);
        for (i, (name, on)) in o.chain.iter().enumerate() {
            let join = o.joins.get(i);
            if !o.filters_only
                && let Some(j) = join
            {
                let said = match &j.join {
                    Some(Join::Converted { from, to }) => {
                        Some((format!("target difference: {from} \u{2192} {to}"), muted))
                    }
                    Some(Join::Refitted { to, .. }) => {
                        Some((format!("correction fitted to {to} for it"), muted))
                    }
                    Some(Join::Unknown) => Some((
                        match &j.suggestion {
                            Some((_, n)) => format!("made against: not set; looks made for {n}"),
                            None => "made against: not set".into(),
                        },
                        muted,
                    )),
                    _ => None,
                };
                if let Some(said) = said {
                    out.push(Line::text(vec![("\u{2502}     ".into(), line_style), said]));
                }
                if let Some(note) = &j.note {
                    out.push(Line::text(vec![
                        ("\u{2502}     ".into(), line_style),
                        (
                            format!("\u{26a0} {note}"),
                            Style::new().fg(theme.tokens.bad),
                        ),
                    ]));
                }
            }
            let left_out = o.left_out_eqs.contains(name) || baked;
            let mut detail = Vec::new();
            if !on {
                detail.push("off".to_owned());
            } else if left_out {
                detail.push("left out".to_owned());
            }
            if !o.filters_only
                && let Some(made) = join.and_then(|j| j.made_for.as_ref())
            {
                let mark = if join.and_then(|j| j.join.as_ref()) == Some(&Join::Matched) {
                    " \u{2713}"
                } else {
                    ""
                };
                detail.push(format!("made for {made}{mark}"));
            }
            let mut l = Line::row(
                Row::Eq(i),
                vec![
                    ("\u{251c}\u{2500} ".into(), line_style),
                    (format!("eq {:<2}", i + 1), muted),
                    (if *on { " [x] " } else { " [ ] " }.into(), ink),
                    (
                        name.clone(),
                        if *on && !left_out {
                            ink.add_modifier(Modifier::BOLD)
                        } else {
                            muted
                        },
                    ),
                    (
                        if detail.is_empty() {
                            String::new()
                        } else {
                            format!("  {}", detail.join(" \u{b7} "))
                        },
                        muted,
                    ),
                ],
            );
            if *on {
                l.thumb = Some((name.clone(), stage_color(theme, i)));
            }
            out.push(l);
        }
        if !baked {
            out.push(Line::row(
                Row::AddEq,
                vec![
                    ("\u{251c}\u{2500} ".into(), line_style),
                    ("+ add EQ".into(), accent),
                ],
            ));
        }
        out.push(Line::text(vec![
            ("\u{2514}\u{2500} ".into(), line_style),
            (format!("{label} out"), muted),
        ]));
        if let Some(left) = &o.left_out {
            out.push(Line::text(vec![(
                format!("\u{26a0} {left}"),
                Style::new().fg(theme.tokens.bad),
            )]));
        }
        out.push(Line::text(Vec::new()));
        out.push(Line::row(
            Row::Manage,
            vec![
                ("manage EQ\u{2026}".into(), accent),
                ("  every correction, EQ and preset".into(), muted),
            ],
        ));
        out
    }

    fn profile_lines(&self, s: &Snapshot, theme: &Theme) -> Vec<Line> {
        let muted = theme.hint_desc;
        let ink = theme.track_normal;
        let accent = Style::new().fg(theme.tokens.accent);
        let bad = Style::new().fg(theme.tokens.bad);
        let key = |k: &str| (format!("{k:<14}"), muted);
        let mut out = Vec::new();
        let Some(p) = self.profile() else {
            out.push(Line::text(vec![("reading\u{2026}".into(), muted)]));
            return out;
        };
        let d = &p.detail;
        if let Some(c) = &d.corrects {
            out.push(Line::text(vec![
                key("corrects"),
                (c.clone(), ink),
                (
                    if d.corrects_baked {
                        "  with a tuning in it".into()
                    } else {
                        String::new()
                    },
                    muted,
                ),
            ]));
        }
        if !d.tunings.is_empty() {
            out.push(Line::text(vec![
                key("tuned with"),
                (d.tunings.join(", "), ink),
            ]));
        }
        if let Some(twice) = &d.corrects_twice {
            out.push(Line::text(vec![(format!("\u{26a0} {twice}"), bad)]));
        }
        if let Some(problem) = &d.problem {
            out.push(Line::text(vec![(format!("\u{26a0} {problem}"), bad)]));
        }
        let own = d.layers.is_empty() || !d.filters.is_empty() || !d.impulses.is_empty();
        if own {
            out.push(Line::row(
                Row::Role,
                vec![key("this is"), (role_label(d.role).into(), ink)],
            ));
            if d.role == DspRole::Tuning {
                let made = d
                    .tuned_for
                    .as_ref()
                    .map(|id| {
                        p.made_against
                            .iter()
                            .find(|(i, _)| i == id)
                            .map_or_else(|| id.clone(), |(_, n)| n.clone())
                    })
                    .unwrap_or_else(|| "unknown".into());
                out.push(Line::row(
                    Row::MadeAgainst,
                    vec![key("made against"), (made, ink)],
                ));
            }
            if d.role == DspRole::Correction
                && let Some(t) = &p.targets
            {
                let id = t
                    .chosen
                    .clone()
                    .or_else(|| t.made_for.map(|m| m.id.to_owned()));
                let name = t
                    .choices
                    .iter()
                    .find(|c| Some(&c.id) == id.as_ref())
                    .map_or_else(|| "unknown".into(), |c| c.name.clone());
                out.push(Line::row(
                    Row::CorrectedTo,
                    vec![key("corrected to"), (name, ink)],
                ));
            }
        }
        if d.group {
            let playing = d
                .layers
                .iter()
                .find(|l| l.on)
                .or(d.layers.first())
                .map(|l| l.profile.clone())
                .unwrap_or_default();
            out.push(Line::row(
                Row::Member,
                vec![
                    key("playing"),
                    (playing, ink),
                    ("  one of the group at a time".into(), muted),
                ],
            ));
        } else if !d.layers.is_empty() {
            let layers: Vec<String> = d
                .layers
                .iter()
                .map(|l| {
                    if l.on {
                        l.profile.clone()
                    } else {
                        format!("{} (off)", l.profile)
                    }
                })
                .collect();
            out.push(Line::text(vec![
                key("plays first"),
                (layers.join(", "), ink),
            ]));
        }
        if let Some(layout) = d.channels.filter(|_| !d.read_only) {
            out.push(Line::row(
                Row::Channels,
                vec![
                    key("channels"),
                    (
                        if layout.stereo {
                            "stereo: bands for each channel"
                        } else {
                            "mono: one list for both"
                        }
                        .into(),
                        ink,
                    ),
                ],
            ));
            if layout.stereo {
                out.push(Line::row(
                    Row::Editing,
                    vec![
                        key("editing"),
                        (if self.channel == 0 { "left" } else { "right" }.into(), ink),
                    ],
                ));
            }
        }
        out.push(Line::text(Vec::new()));
        let filters = self.filters();
        if !filters.is_empty() {
            out.push(Line::text(vec![(
                format!(
                    " {:<3} {:<12} {:>8} {:>7} {:>6}",
                    "#", "type", "Hz", "dB", "Q"
                ),
                muted,
            )]));
        }
        for (n, i) in self.shown_filters().into_iter().enumerate() {
            let f = &filters[i];
            let text = match f {
                DspFilter::Band(b) if KINDS.iter().any(|(k, _)| *k == b.kind) => format!(
                    " {:<3} {:<12} {:>8} {:>7} {:>6}",
                    n + 1,
                    kind_label(b.kind),
                    hz_label(b.freq),
                    format!("{:+.1}", b.gain_db),
                    format!("{:.2}", b.q)
                ),
                other => format!(" {:<3} {}", n + 1, filter_label(other)),
            };
            out.push(Line::row(Row::Band(i), vec![(text, ink)]));
        }
        if !d.read_only && d.impulses.is_empty() && (d.layers.is_empty() || !d.filters.is_empty()) {
            out.push(Line::row(
                Row::AddBand,
                vec![("+ add a band".into(), accent)],
            ));
        }
        if d.read_only && !filters.is_empty() {
            out.push(Line::text(vec![(
                "a correction plays as made; to change the sound, add an EQ on top".into(),
                muted,
            )]));
        }
        for ir in &d.impulses {
            let channels = match ir.channels {
                None => "every channel".to_owned(),
                Some(1) => "mono".into(),
                Some(2) => "stereo".into(),
                Some(n) => format!("{n} channels"),
            };
            out.push(Line::text(vec![
                key("impulse"),
                (
                    format!(
                        "{} kHz \u{b7} {channels} \u{b7} {} taps",
                        khz(ir.rate),
                        ir.taps
                    ),
                    ink,
                ),
                (format!("  {}", ir.file), muted),
            ]));
        }
        out.push(Line::text(vec![
            key("preamp"),
            (format!("{:+.1} dB", d.preamp_db), ink),
            (
                if d.preamp_set {
                    "  set in this EQ".into()
                } else {
                    format!(
                        "  derived at {} kHz so nothing it boosts can clip",
                        khz(d.preamp_rate)
                    )
                },
                muted,
            ),
        ]));
        out.push(Line::text(Vec::new()));
        let used: Vec<String> = d.devices.iter().map(|x| s.label(x)).collect();
        if let Some(in_use) = s.in_use.as_deref() {
            let label = s.label(in_use);
            let using = d.devices.iter().any(|x| x == in_use)
                || (d.role == DspRole::Tuning
                    && s.device.as_deref() == Some(in_use)
                    && s.overview.chain.iter().any(|(n, _)| *n == d.name));
            out.push(Line::row(
                Row::Use,
                vec![
                    key("used for"),
                    (
                        if used.is_empty() {
                            "no output yet".into()
                        } else {
                            used.join(", ")
                        },
                        ink,
                    ),
                    (
                        if using && d.role != DspRole::Tuning {
                            format!("  stop using for {label}")
                        } else if using {
                            String::new()
                        } else {
                            format!("  use for {label}")
                        },
                        accent,
                    ),
                ],
            ));
        }
        out.push(Line::row(
            Row::Sync,
            vec![
                key("sync"),
                (
                    if d.everywhere {
                        "every device"
                    } else {
                        "this device"
                    }
                    .into(),
                    ink,
                ),
                (
                    if d.everywhere {
                        "  kept on every device signed in to your koan server"
                    } else {
                        "  never leaves this one"
                    }
                    .into(),
                    muted,
                ),
            ],
        ));
        if !d.source.is_empty() {
            out.push(Line::text(vec![
                key("imported from"),
                (d.source.join(", "), muted),
            ]));
        }
        out.push(Line::text(Vec::new()));
        out.push(Line::row(
            Row::Rename,
            vec![("rename\u{2026}".into(), accent)],
        ));
        out.push(Line::row(
            Row::Copy,
            vec![("save as new\u{2026}".into(), accent)],
        ));
        if d.can_revert && d.edited {
            out.push(Line::row(
                Row::Reset,
                vec![("reset to file".into(), accent)],
            ));
        }
        out.push(Line::row(Row::Delete, vec![("delete\u{2026}".into(), bad)]));
        out
    }

    // --- Drawing ----------------------------------------------------------

    pub fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        self.hits.clear();
        self.rows.clear();
        if area.width < 4 || area.height < 2 {
            return;
        }
        let title = match self.view() {
            View::Chain => "eq".to_owned(),
            View::Manage => "eq \u{b7} manage".to_owned(),
            View::Profile(name) => format!("eq \u{b7} {name}"),
        };
        let note = match (self.view(), self.profile()) {
            (View::Profile(_), Some(p)) => {
                let mut n = format!("  {}", role_label(p.detail.role));
                if p.detail.edited {
                    n.push_str(" \u{b7} edited");
                }
                n
            }
            (View::Chain, _) => self
                .snap
                .as_ref()
                .filter(|s| !s.overview.enabled)
                .map(|_| "  processing is off".into())
                .unwrap_or_default(),
            _ => String::new(),
        };
        super::ui::heading(buf, area, theme, &title, &note);
        if self.views.len() > 1 {
            self.hits.push((
                Rect::new(area.x, area.y, title.chars().count() as u16, 1),
                Hit::Back,
            ));
        }
        let body = Rect::new(
            area.x + 1,
            area.y + 2,
            area.width.saturating_sub(2),
            area.height.saturating_sub(2),
        );
        if body.height == 0 || body.width == 0 {
            return;
        }
        let lines = self.lines(theme);
        let footer = self.footer(theme);
        let footer_h = if body.height >= 14 {
            footer.len().min(3) as u16
        } else {
            0
        };
        // The graph, when the page has room for it and the rows.
        let graph_h = {
            let spare = body.height.saturating_sub(footer_h + 6);
            let want = (body.height * 2 / 5).clamp(6, 16);
            if spare >= 6 { want.min(spare) } else { 0 }
        };
        if graph_h > 0 {
            self.render_graph(Rect::new(body.x, body.y, body.width, graph_h), buf, theme);
        }
        let list = Rect::new(
            body.x,
            body.y + graph_h + u16::from(graph_h > 0),
            body.width,
            body.height
                .saturating_sub(graph_h + u16::from(graph_h > 0) + footer_h),
        );
        self.render_lines(&lines, list, buf, theme);
        if footer_h > 0 {
            let y = body.y + body.height - footer_h;
            for (i, line) in footer.iter().take(footer_h as usize).enumerate() {
                let w = body.width as usize;
                buf.set_stringn(body.x, y + i as u16, fit(&line.0, w), w, line.1);
            }
        }
        if let Some(p) = &self.prompt {
            self.render_prompt(p, area, buf, theme);
        } else if self.choice.is_some() {
            self.render_choice(area, buf, theme);
        }
    }

    /// What the bottom of the page says: the last change's word, then the
    /// chain in words.
    fn footer(&self, theme: &Theme) -> Vec<(String, Style)> {
        let mut out = Vec::new();
        if let Some((m, bad)) = &self.message {
            out.push((
                m.clone(),
                if *bad {
                    Style::new().fg(theme.tokens.bad)
                } else {
                    theme.hint_desc
                },
            ));
        }
        if *self.view() == View::Chain
            && let Some(s) = &self.snap
        {
            out.push((s.sentence.clone(), theme.hint_desc));
        }
        out
    }

    fn render_graph(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let Some(s) = self.snap.as_deref() else {
            return;
        };
        let flat_db = [0.0, 0.0];
        let flat_hz = [20.0, 20_000.0];
        match self.view() {
            View::Profile(_) => {
                let Some(p) = self.profile() else { return };
                let Some(r) = p.response.as_ref() else {
                    return;
                };
                let stereo = p.detail.channels.is_some_and(|c| c.stereo);
                let filters = self.filters();
                let selected = match self.row_list().get(self.cursor) {
                    Some(Row::Band(i)) => Some(*i),
                    _ => None,
                };
                // A band edited ahead of the read: its shape, from the
                // figures shown, so the graph follows the keys at once.
                let local_band: Vec<f64> = selected
                    .and_then(|i| filters.get(i))
                    .filter(|f| matches!(f, DspFilter::Band(b) if b.kind != EqFilterKind::Gain))
                    .map(|f| {
                        koan_core::audio::dsp::response(
                            std::slice::from_ref(f),
                            &r.freqs,
                            super::eq_worker::RATE,
                        )
                    })
                    .unwrap_or_default();
                let level = r.preamp_db + r.gain_db;
                let total: Vec<f64> = r.total.iter().map(|v| v + r.preamp_db).collect();
                let right: Vec<f64> = r
                    .right
                    .as_ref()
                    .map(|db| db.iter().map(|v| v + r.preamp_db).collect())
                    .unwrap_or_default();
                let band: Vec<f64> = local_band.iter().map(|v| v + level).collect();
                let mut curves = Vec::new();
                if !band.is_empty() {
                    curves.push(Curve {
                        db: &band,
                        color: theme.tokens.muted,
                        dash: eq_graph::DASHES[1],
                    });
                }
                if stereo && !right.is_empty() {
                    curves.push(Curve {
                        db: if self.channel == 0 {
                            right.as_slice()
                        } else {
                            total.as_slice()
                        },
                        color: theme.tokens.muted,
                        dash: eq_graph::DASHES[0],
                    });
                }
                curves.push(Curve {
                    db: if stereo && self.channel == 1 && !right.is_empty() {
                        right.as_slice()
                    } else {
                        total.as_slice()
                    },
                    color: theme.tokens.accent,
                    dash: eq_graph::SOLID,
                });
                let handles = if p.detail.read_only {
                    Vec::new()
                } else {
                    filters
                        .iter()
                        .enumerate()
                        .filter_map(|(i, f)| match f {
                            DspFilter::Band(b)
                                if matches!(
                                    b.kind,
                                    EqFilterKind::Peaking
                                        | EqFilterKind::LowShelf
                                        | EqFilterKind::HighShelf
                                ) && (b.channels.is_empty()
                                    || b.channels.contains(&self.channel)) =>
                            {
                                Some(Handle {
                                    hz: b.freq,
                                    db: b.gain_db + level,
                                    selected: selected == Some(i),
                                })
                            }
                            _ => None,
                        })
                        .collect()
                };
                EqGraph {
                    freqs: &r.freqs,
                    curves,
                    handles,
                    level,
                    note: None,
                }
                .render(area, buf, theme);
            }
            _ => {
                let o = &s.overview;
                let flat = o.active.is_none() && o.chain.is_empty();
                match s.response.as_ref().filter(|_| !flat) {
                    Some(r) => {
                        let mut curves = Vec::new();
                        let playing: Vec<(usize, &str)> = o
                            .chain
                            .iter()
                            .enumerate()
                            .filter(|(_, (n, on))| *on && !o.left_out_eqs.contains(n))
                            .map(|(i, (n, _))| (i, n.as_str()))
                            .collect();
                        if o.active.is_some()
                            && let Some(db) = r
                                .correction
                                .as_deref()
                                .or((playing.is_empty()).then_some(r.total.as_slice()))
                        {
                            curves.push(Curve {
                                db,
                                color: theme.tokens.accent,
                                dash: eq_graph::SOLID,
                            });
                        }
                        for (i, name) in &playing {
                            if let Some(layer) = r.layers.iter().find(|l| l.name == *name) {
                                curves.push(Curve {
                                    db: &layer.db,
                                    color: stage_color(theme, *i),
                                    dash: eq_graph::DASHES[*i % eq_graph::DASHES.len()],
                                });
                            }
                        }
                        if let Some(right) = &r.right {
                            curves.push(Curve {
                                db: right,
                                color: theme.tokens.muted,
                                dash: eq_graph::DASHES[0],
                            });
                        }
                        curves.push(Curve {
                            db: &r.total,
                            color: theme.tokens.strong,
                            dash: eq_graph::SOLID,
                        });
                        EqGraph {
                            freqs: &r.freqs,
                            curves,
                            handles: Vec::new(),
                            level: 0.0,
                            note: None,
                        }
                        .render(area, buf, theme);
                    }
                    None => EqGraph {
                        freqs: &flat_hz,
                        curves: vec![Curve {
                            db: &flat_db,
                            color: theme.tokens.strong,
                            dash: eq_graph::SOLID,
                        }],
                        handles: Vec::new(),
                        level: 0.0,
                        note: Some("flat: plays untouched"),
                    }
                    .render(area, buf, theme),
                }
            }
        }
    }

    fn render_lines(&mut self, lines: &[Line], area: Rect, buf: &mut Buffer, theme: &Theme) {
        if area.height == 0 {
            return;
        }
        let rows: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.row.is_some())
            .map(|(i, _)| i)
            .collect();
        self.cursor = self.cursor.min(rows.len().saturating_sub(1));
        let cursor_line = rows.get(self.cursor).copied().unwrap_or(0);
        let h = area.height as usize;
        if cursor_line < self.scroll {
            self.scroll = cursor_line;
        } else if cursor_line >= self.scroll + h {
            self.scroll = cursor_line + 1 - h;
        }
        self.scroll = self.scroll.min(lines.len().saturating_sub(h));
        self.rows = lines.iter().map(|l| l.row.clone()).collect();
        let selected_style = theme.track_cursor;
        let freqs = self
            .snap
            .as_ref()
            .and_then(|s| s.response.as_ref().map(|r| r.freqs.clone()))
            .unwrap_or_default();
        for (i, line) in lines.iter().enumerate().skip(self.scroll).take(h) {
            let y = area.y + (i - self.scroll) as u16;
            let selected = line.row.is_some()
                && i == cursor_line
                && self.choice.is_none()
                && self.prompt.is_none();
            if selected {
                for x in area.x..area.x + area.width {
                    buf[(x, y)].set_style(selected_style);
                }
            }
            let right_w: u16 = line
                .right
                .iter()
                .map(|(t, _)| t.chars().count() as u16)
                .sum();
            let thumb_w = if line.thumb.is_some() && area.width >= 50 {
                10u16
            } else {
                0
            };
            let end = area.x
                + area
                    .width
                    .saturating_sub(right_w + thumb_w + u16::from(right_w + thumb_w > 0));
            let mut x = area.x;
            for (text, style) in &line.spans {
                if x >= end {
                    break;
                }
                let w = (end - x) as usize;
                let t = fit(text, w);
                let style = if selected {
                    style.patch(selected_style)
                } else {
                    *style
                };
                buf.set_stringn(x, y, &t, w, style);
                x += t.chars().count() as u16;
            }
            if let (Some((name, color)), true) = (&line.thumb, thumb_w > 0)
                && let Some(db) = self.snap.as_ref().and_then(|s| s.curve(name))
            {
                eq_graph::thumb(
                    Rect::new(end + 1, y, thumb_w - 2, 1),
                    buf,
                    &freqs,
                    db,
                    *color,
                );
            }
            let mut rx = area.x + area.width - right_w;
            for (text, style) in &line.right {
                let style = if selected {
                    style.patch(selected_style)
                } else {
                    *style
                };
                buf.set_string(rx, y, text, style);
                rx += text.chars().count() as u16;
            }
            let row = Rect::new(area.x, y, area.width, 1);
            if let Some(Row::Band(index)) = line.row {
                // Each figure its own target, and the one selected marked.
                let editable = self.editable() && self.band_editable(index);
                for (field, x0, w) in [
                    (Field::Kind, 5u16, 12u16),
                    (Field::Freq, 18, 8),
                    (Field::Gain, 27, 7),
                    (Field::Q, 35, 6),
                ] {
                    if x0 + w > area.width {
                        continue;
                    }
                    let cell = Rect::new(area.x + x0, y, w, 1);
                    if editable {
                        self.hits.push((cell, Hit::Field(i, index, field)));
                    }
                    let typing = self
                        .typing
                        .as_ref()
                        .filter(|t| t.index == index && t.field == field);
                    if let Some(t) = typing {
                        let text = format!("{:>w$}", format!("{}\u{2581}", t.text), w = w as usize);
                        buf.set_stringn(
                            cell.x,
                            y,
                            fit(&text, w as usize),
                            w as usize,
                            theme.picker_cursor.add_modifier(Modifier::BOLD),
                        );
                    } else if selected && editable && self.field == field {
                        for cx in cell.x..cell.x + cell.width {
                            buf[(cx, y)].set_style(theme.picker_cursor);
                        }
                    }
                }
            }
            if line.row.is_some() {
                self.hits.push((row, Hit::Line(i)));
            }
        }
    }

    fn render_choice(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let Some(c) = self.choice.as_ref() else {
            return;
        };
        let w = 64u16.min(area.width.saturating_sub(2));
        let note_lines = c
            .note
            .as_deref()
            .map(|n| wrap(n, w.saturating_sub(4) as usize))
            .unwrap_or_default();
        let item_h: u16 = c
            .items
            .iter()
            .map(|i| 1 + u16::from(!i.detail.is_empty()))
            .sum();
        let h = (2 + note_lines.len() as u16 + u16::from(!note_lines.is_empty()) + item_h)
            .min(area.height.saturating_sub(1));
        if w < 12 || h < 3 {
            return;
        }
        let x = area.x + (area.width - w) / 2;
        let y = area.y + (area.height - h) / 2;
        let popup = Rect::new(x, y, w, h);
        self.hits.push((area, Hit::Outside));
        self.hits.push((popup, Hit::Inside));
        let mut items = Vec::new();
        box_frame(popup, buf, theme, &c.title);
        let inner = Rect::new(x + 2, y + 1, w.saturating_sub(4), h.saturating_sub(2));
        let mut cy = inner.y;
        for n in &note_lines {
            if cy >= inner.y + inner.height {
                break;
            }
            buf.set_stringn(inner.x, cy, n, inner.width as usize, theme.hint_desc);
            cy += 1;
        }
        if !note_lines.is_empty() {
            cy += 1;
        }
        // Keep the cursor's item in view.
        let heights: Vec<u16> = c
            .items
            .iter()
            .map(|i| 1 + u16::from(!i.detail.is_empty()))
            .collect();
        let room = (inner.y + inner.height).saturating_sub(cy);
        let mut first = 0;
        let last = c.cursor.min(heights.len().saturating_sub(1));
        while first < last && heights[first..=last].iter().sum::<u16>() > room {
            first += 1;
        }
        for (i, item) in c.items.iter().enumerate().skip(first) {
            let ih = 1 + u16::from(!item.detail.is_empty());
            if cy + ih > inner.y + inner.height {
                break;
            }
            let selected = i == c.cursor;
            let style = if selected {
                theme.picker_cursor
            } else {
                theme.track_normal
            };
            if selected {
                for row in cy..cy + ih {
                    for cx in popup.x + 1..popup.x + popup.width - 1 {
                        buf[(cx, row)].set_style(style);
                    }
                }
            }
            let mark = if item.chosen { "\u{2713} " } else { "  " };
            let mark_style = if selected {
                style
            } else {
                Style::new().fg(theme.tokens.accent)
            };
            buf.set_string(inner.x, cy, mark, mark_style);
            let lw = inner.width.saturating_sub(2) as usize;
            buf.set_stringn(inner.x + 2, cy, fit(&item.label, lw), lw, style);
            if !item.detail.is_empty() {
                let dstyle = if selected { style } else { theme.hint_desc };
                buf.set_stringn(inner.x + 2, cy + 1, fit(&item.detail, lw), lw, dstyle);
            }
            items.push((
                Rect::new(popup.x + 1, cy, popup.width - 2, ih),
                Hit::Item(i),
            ));
            cy += ih;
        }
        self.hits.extend(items);
    }

    fn render_prompt(&self, p: &Prompt, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let w = 64u16.min(area.width.saturating_sub(2));
        let note = wrap(&p.note, w.saturating_sub(4) as usize);
        let h = (5 + note.len() as u16).min(area.height);
        if w < 12 || h < 3 {
            return;
        }
        let x = area.x + (area.width - w) / 2;
        let y = area.y + (area.height - h) / 2;
        let popup = Rect::new(x, y, w, h);
        box_frame(popup, buf, theme, &p.title);
        let inner_w = w.saturating_sub(4) as usize;
        let mut cy = y + 1;
        for n in &note {
            if cy >= y + h - 1 {
                break;
            }
            buf.set_stringn(x + 2, cy, n, inner_w, theme.hint_desc);
            cy += 1;
        }
        cy += 1;
        if cy < y + h - 1 {
            // The end of what is typed, where a long path keeps going.
            let shown: String = {
                let chars: Vec<char> = p.text.replace('\n', " \u{21b5} ").chars().collect();
                let keep = inner_w.saturating_sub(1);
                chars[chars.len().saturating_sub(keep)..].iter().collect()
            };
            buf.set_stringn(
                x + 2,
                cy,
                format!("{shown}\u{2581}"),
                inner_w,
                theme.track_normal.add_modifier(Modifier::BOLD),
            );
        }
    }
}

/// A frame with a title, cleared inside.
fn box_frame(area: Rect, buf: &mut Buffer, theme: &Theme, title: &str) {
    use ratatui::widgets::{Block, Borders, Clear, Widget};
    Clear.render(area, buf);
    Block::default()
        .borders(Borders::ALL)
        .border_style(theme.rule)
        .title(ratatui::text::Span::styled(
            format!(" {title} "),
            theme.heading,
        ))
        .render(area, buf);
}

/// The chain with `name` on the end, switched on, where it is not in it.
fn profiles_chain_with(chain: &[(String, bool)], name: &str) -> Vec<(String, bool)> {
    let mut chain = chain.to_vec();
    if !chain.iter().any(|(n, _)| n == name) {
        chain.push((name.to_owned(), true));
    }
    chain
}

/// The first EQ that plays meets the correction matched, and none after it
/// was made against another target: the line through the chain is drawn in
/// the accent.
fn chain_matched(s: &Snapshot) -> bool {
    let o = &s.overview;
    let Some(c) = o
        .profiles
        .iter()
        .find(|p| Some(&p.name) == o.active.as_ref())
    else {
        return false;
    };
    if c.role == DspRole::Baked || !o.tuning_plays {
        return false;
    }
    let mut playing = o
        .chain
        .iter()
        .zip(&o.joins)
        .filter(|((n, on), _)| *on && !o.left_out_eqs.contains(n))
        .map(|(_, j)| j.join.as_ref());
    match playing.next() {
        Some(Some(Join::Matched)) => {
            playing.all(|j| matches!(j, Some(Join::Matched) | Some(Join::Unknown)))
        }
        _ => false,
    }
}

fn manage_lines(s: &Snapshot, theme: &Theme) -> Vec<Line> {
    let o = &s.overview;
    let muted = theme.hint_desc;
    let ink = theme.track_normal;
    let accent = Style::new().fg(theme.tokens.accent);
    let heading = theme.heading;
    let grouped: Vec<&String> = o.profiles.iter().flat_map(|p| p.members.iter()).collect();
    let loose: Vec<&Summary> = o
        .profiles
        .iter()
        .filter(|p| !p.preset && p.members.is_empty() && !grouped.contains(&&p.name))
        .collect();
    let row = |p: &Summary| {
        let used = if p.used_on.is_empty() {
            "not used".to_owned()
        } else {
            let on: Vec<String> = p.used_on.iter().map(|d| s.label(d)).collect();
            format!("used on {}", on.join(", "))
        };
        let what = match &p.problem {
            Some(problem) => (problem.clone(), Style::new().fg(theme.tokens.bad)),
            None => (
                [describe(p), used]
                    .into_iter()
                    .filter(|t| !t.is_empty())
                    .collect::<Vec<_>>()
                    .join(" \u{b7} "),
                muted,
            ),
        };
        Line::row(
            Row::Profile(p.name.clone()),
            vec![
                ("  ".into(), ink),
                (p.name.clone(), ink.add_modifier(Modifier::BOLD)),
                (
                    if p.edited {
                        "  edited".into()
                    } else {
                        String::new()
                    },
                    muted,
                ),
                (format!("  {}", what.0), what.1),
            ],
        )
    };
    let mut out = Vec::new();
    let section = |title: &str, items: Vec<&Summary>, out: &mut Vec<Line>| {
        if items.is_empty() {
            return;
        }
        out.push(Line::text(vec![(title.to_owned(), heading)]));
        out.extend(items.into_iter().map(row));
        out.push(Line::text(Vec::new()));
    };
    section(
        "corrections",
        loose
            .iter()
            .copied()
            .filter(|p| p.role != DspRole::Tuning)
            .collect(),
        &mut out,
    );
    section(
        "EQs",
        loose
            .iter()
            .copied()
            .filter(|p| p.role == DspRole::Tuning)
            .collect(),
        &mut out,
    );
    for g in o.profiles.iter().filter(|p| !p.members.is_empty()) {
        let members: Vec<&Summary> = g
            .members
            .iter()
            .filter_map(|m| o.profiles.iter().find(|p| p.name == *m))
            .collect();
        out.push(row(g));
        out.extend(members.into_iter().map(|m| {
            let mut l = row(m);
            l.spans[0].0 = "    ".into();
            l
        }));
        out.push(Line::text(Vec::new()));
    }
    section(
        "presets",
        o.profiles.iter().filter(|p| p.preset).collect(),
        &mut out,
    );
    if o.profiles.is_empty() {
        out.push(Line::text(vec![("no EQ yet".into(), muted)]));
        out.push(Line::text(Vec::new()));
    }
    out.push(Line::row(
        Row::New,
        vec![("+ new EQ\u{2026}".into(), accent)],
    ));
    out.push(Line::row(
        Row::Import,
        vec![
            ("+ import a file\u{2026}".into(), accent),
            ("  or drop one on the terminal".into(), muted),
        ],
    ));
    out
}

/// What a profile holds, in a few words, as the apps describe it.
fn describe(p: &Summary) -> String {
    let mut parts: Vec<String> = Vec::new();
    if p.measured {
        parts.push("from a measurement".into());
    }
    if !p.members.is_empty() {
        parts.push(format!("{} to pick from", p.members.len()));
    } else if p.preset {
        parts.push(if p.layers == 1 {
            "1 part".into()
        } else {
            format!("{} parts", p.layers)
        });
    } else if p.layers > 0 {
        parts.push(format!(
            "plays {} {} in order",
            p.layers,
            if p.layers == 1 { "EQ" } else { "EQs" }
        ));
    }
    if p.graphics > 0 {
        parts.push(format!(
            "graphic, {} {}",
            p.points,
            if p.points == 1 { "point" } else { "points" }
        ));
    }
    let bands = p.bands - p.graphics;
    if bands > 0 {
        parts.push(format!(
            "{bands} {}",
            if bands == 1 { "band" } else { "bands" }
        ));
    }
    if !p.rates.is_empty() {
        let rates: Vec<String> = p.rates.iter().map(|r| khz(*r)).collect();
        parts.push(format!("{} kHz", rates.join(", ")));
    }
    parts.join(" \u{b7} ")
}

fn role_label(role: DspRole) -> &'static str {
    match role {
        DspRole::Correction => "correction",
        DspRole::Tuning => "tuning",
        DspRole::Baked => "correction + tuning",
    }
}

fn kind_label(kind: EqFilterKind) -> &'static str {
    KINDS
        .iter()
        .find(|(k, _)| *k == kind)
        .map_or_else(|| kind.name(), |(_, l)| l)
}

/// Each EQ of the chain drawn in a colour of its own, the same in its row
/// and on the graph.
fn stage_color(theme: &Theme, i: usize) -> Color {
    [
        theme.tokens.ink,
        theme.warning,
        theme.tokens.muted,
        theme.tokens.strong,
    ][i % 4]
}

fn khz(hz: u32) -> String {
    let k = hz as f64 / 1000.0;
    if k == k.round() {
        format!("{k:.0}")
    } else {
        format!("{k:.1}")
    }
}

/// A frequency as the table shows it: whole hertz.
fn hz_label(hz: f64) -> String {
    if hz < 100.0 && hz.fract().abs() > 0.05 {
        format!("{hz:.1}")
    } else {
        format!("{hz:.0}")
    }
}

/// A filter the table does not edit, in a line.
fn filter_label(f: &DspFilter) -> String {
    let channel = |c: u16| match c {
        0 => "L".to_owned(),
        1 => "R".to_owned(),
        n => format!("ch {}", n + 1),
    };
    let on = |chs: &[u16]| {
        if chs.is_empty() {
            String::new()
        } else {
            format!(
                " \u{b7} {}",
                chs.iter()
                    .map(|c| channel(*c))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        }
    };
    match f {
        DspFilter::Band(b) => {
            let name = match b.kind {
                EqFilterKind::LowShelfFirstOrder => "low shelf, 6 dB/oct",
                EqFilterKind::HighShelfFirstOrder => "high shelf, 6 dB/oct",
                EqFilterKind::LowPassFirstOrder => "low pass, 6 dB/oct",
                EqFilterKind::HighPassFirstOrder => "high pass, 6 dB/oct",
                EqFilterKind::AllPassFirstOrder => "all pass, first order",
                EqFilterKind::Gain => "gain",
                k => kind_label(k),
            };
            match b.kind {
                EqFilterKind::Gain => format!("{name}  {:+.1} dB{}", b.gain_db, on(&b.channels)),
                _ => format!(
                    "{name}  {} Hz  {:+.1} dB{}",
                    hz_label(b.freq),
                    b.gain_db,
                    on(&b.channels)
                ),
            }
        }
        DspFilter::Delay(d) => {
            let mut parts = Vec::new();
            if d.ms != 0.0 {
                parts.push(format!("{:.2} ms", d.ms));
            }
            if d.samples != 0.0 {
                parts.push(format!("{:.2} samples", d.samples));
            }
            format!("delay  {}{}", parts.join(" + "), on(&d.channels))
        }
        DspFilter::Mix(m) => {
            let terms: Vec<String> = m
                .outputs
                .iter()
                .enumerate()
                .filter(|(o, s)| !(s.len() == 1 && s[0].0 as usize == *o && s[0].1 == 1.0))
                .map(|(o, s)| {
                    let src: Vec<String> = s
                        .iter()
                        .map(|(c, g)| {
                            if *g == 1.0 {
                                channel(*c)
                            } else {
                                format!("{g}\u{d7}{}", channel(*c))
                            }
                        })
                        .collect();
                    format!(
                        "{} = {}",
                        channel(o as u16),
                        if src.is_empty() {
                            "0".into()
                        } else {
                            src.join(" + ")
                        }
                    )
                })
                .collect();
            format!("mix  {}", terms.join("   "))
        }
        DspFilter::Graphic(g) => {
            let (lo, hi) = g
                .points
                .iter()
                .fold((f64::MAX, f64::MIN), |(lo, hi), (_, db)| {
                    (lo.min(*db), hi.max(*db))
                });
            if g.points.is_empty() {
                "graphic EQ".into()
            } else {
                format!(
                    "graphic EQ  {} points  {lo:+.1} to {hi:+.1} dB{}",
                    g.points.len(),
                    on(&g.channels)
                )
            }
        }
    }
}

/// A frequency typed: "1000", "1k", "1.5 kHz", "200hz".
fn parse_hz(text: &str) -> Option<f64> {
    let t = text.trim().to_lowercase().replace(' ', "");
    let t = t.trim_end_matches("hz");
    let (num, mul) = match t.strip_suffix('k') {
        Some(n) => (n, 1000.0),
        None => (t, 1.0),
    };
    num.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .map(|v| v * mul)
}

/// A number typed, with its unit, if any, after it.
fn parse_number(text: &str, unit: &str) -> Option<f64> {
    let t = text.trim().to_lowercase().replace(' ', "");
    let t = t.trim_end_matches(unit).trim_start_matches(unit);
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Paths typed or pasted: one a line, or several on a line quoted or with
/// their spaces escaped, as a terminal drops them; `~` is home.
pub fn parse_paths(text: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if std::path::Path::new(&expand(line)).exists() {
            out.push(PathBuf::from(expand(line)));
            continue;
        }
        let mut word = String::new();
        let mut quote: Option<char> = None;
        let mut chars = line.chars();
        while let Some(c) = chars.next() {
            match (c, quote) {
                ('\\', _) => {
                    if let Some(n) = chars.next() {
                        word.push(n);
                    }
                }
                ('\'' | '"', None) => quote = Some(c),
                (q, Some(open)) if q == open => quote = None,
                (' ', None) => {
                    if !word.is_empty() {
                        out.push(PathBuf::from(expand(&word)));
                        word.clear();
                    }
                }
                _ => word.push(c),
            }
        }
        if !word.is_empty() {
            out.push(PathBuf::from(expand(&word)));
        }
    }
    out
}

fn expand(path: &str) -> String {
    let path = path.strip_prefix("file://").unwrap_or(path);
    match (path.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) => format!("{home}/{rest}"),
        _ => path.to_owned(),
    }
}

/// `text` cut to `width` cells, with an ellipsis where it was cut.
fn fit(text: &str, width: usize) -> String {
    super::browse::fit(text, width)
}

/// `text` in lines of at most `width`, broken at spaces.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
#[path = "eq_page_tests.rs"]
mod tests;
