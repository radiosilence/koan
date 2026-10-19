//! The Settings page: the apps' Settings, as a list of sections beside the
//! pane of the one chosen. Each change is written through `Config::persist`,
//! which puts it in the file that owns it, and each action goes through the
//! koan-core call the apps' Settings makes. Anything that waits on the disk
//! or the server runs on a thread of its own and lands on a later frame.

mod rows;
mod work;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};
use koan_core::config::{Config, NearbyControl, ReplayGainMode};
use koan_core::remote::client::{
    KoanApiKey, KoanAppPassword, KoanMcp, KoanScrobbleService, KoanUser,
};
use koan_core::remote::connections::{Connection, Held};
use koan_core::remote::profile::ServerProfile;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Widget};
use unicode_width::UnicodeWidthStr;

use super::text::{fit, tail, wrap};
use super::theme::Theme;
use rows::{Kind, Row, Step};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Library,
    Server,
    Account,
    People,
    Playback,
    Eq,
    Devices,
    Integrations,
    Terminal,
}

impl Section {
    fn label(self) -> &'static str {
        match self {
            Section::Library => "library",
            Section::Server => "server",
            Section::Account => "account",
            Section::People => "people",
            Section::Playback => "playback",
            Section::Eq => "eq",
            Section::Devices => "devices",
            Section::Integrations => "integrations",
            Section::Terminal => "terminal",
        }
    }
}

/// A value the page steps through with `←` and `→`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    ReplayGain,
    PreAmp,
    FadeOnPause,
    Theme,
    CoverGraphics,
    ArtSize,
    ShowFps,
    FrameRate,
    Palette,
    AutoSync,
    SyncInterval,
    DownloadWorkers,
    Offline,
    Discoverable,
    NearbyControl,
}

/// Long work on the library, which the app runs one at a time and reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Task {
    Scan {
        force: bool,
    },
    Sync,
    /// Forget the tracks of a folder taken out of the library.
    ForgetFolder(PathBuf),
    /// Forget the tracks only the server had, after signing out.
    ForgetRemote,
    ClearCache,
    RebuildIndex,
}

/// What a change on the page asks of the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// A setting the running TUI applies itself.
    Changed(Setting),
    /// A line for the status bar: what happened, or why it did not.
    Say(String),
    Run(Task),
    OpenEq,
    OpenOutputs,
    OpenUrl(String),
    Copy(String),
    /// The server was signed out of: the player lets go of its queue and
    /// downloads, and its tracks are forgotten too when asked.
    SignedOut {
        forget: bool,
    },
}

/// A text the page asks for, kept as typed until it is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Field {
    Url,
    Username,
    Secret,
    Folder,
    CacheLimit,
    CurrentPassword,
    NewPassword,
    KeyName,
    AppPasswordName,
    PersonName,
    PairCode,
    ScrobbleToken,
    Address,
    Grantee,
}

impl Field {
    fn secret(self) -> bool {
        matches!(
            self,
            Field::Secret | Field::CurrentPassword | Field::NewPassword | Field::ScrobbleToken
        )
    }

    fn placeholder(self) -> &'static str {
        match self {
            Field::Url => "https://music.example.com, or an invite link",
            Field::Username | Field::PersonName => "username",
            Field::Secret => "the account's password or API key",
            Field::Folder => "the path of a folder of music",
            Field::CacheLimit => "e.g. 50GB, blank for no limit",
            Field::CurrentPassword => "current password",
            Field::NewPassword => "new password",
            Field::KeyName | Field::AppPasswordName => "the app it is for",
            Field::PairCode => "XXXX-XXXX",
            Field::ScrobbleToken => "ListenBrainz user token",
            Field::Address => "host or host:port",
            Field::Grantee => "their username on this server",
        }
    }
}

/// An account's access on a koan server, as the server names it.
pub(crate) const ROLES: [(&str, &str); 3] = [
    ("readonly", "listen only"),
    ("user", "listen and edit"),
    ("admin", "admin"),
];

pub(crate) fn role_label(role: &str) -> &'static str {
    ROLES
        .iter()
        .find(|(r, _)| *r == role)
        .map_or("listen only", |(_, l)| l)
}

fn cycle_role(role: &str, forward: bool) -> &'static str {
    let names: Vec<&'static str> = ROLES.iter().map(|(r, _)| *r).collect();
    cycle(&names, role, forward)
}

/// What a row, a menu choice or a dialog's button does.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Op {
    Outcome(Outcome),
    SignIn,
    AskSignOut,
    SignOut {
        forget: bool,
    },
    AddFolder,
    AskRemoveFolder(PathBuf),
    RemoveFolder {
        path: PathBuf,
        forget: bool,
    },
    AskRebuild,
    CommitCacheLimit,
    ChangePassword,
    NewApiKey,
    AskRevokeKey(i64, String),
    RevokeKey(i64),
    NewAppPassword,
    AskRevokeAppPassword(i64, String),
    RevokeAppPassword(i64),
    AddPerson,
    Invite {
        username: String,
        reset: bool,
    },
    AskReset(String),
    AskSetPassword(String),
    /// The password is the dialog's input.
    SetPassword(String),
    AskDelete(String),
    Delete(String),
    /// The access chooser for an account, and its current role.
    AskRole(String, String),
    SetRole(String, String),
    Pair,
    ApprovePair(String),
    DeclinePair(String),
    ConnectScrobbling,
    DisconnectScrobbling,
    EndConnection {
        key: u64,
        refuse: Option<String>,
    },
    AllowHeld(String),
    AllowRefused(String),
    AddAddress,
    RemoveAddress(String),
    Share,
    StopSharing(String),
    Cancel,
    /// A menu of a row's actions.
    Menu(String, Vec<(&'static str, Op)>),
}

/// A question over the page: its choices, and a password to type when it
/// asks for one. `Esc` cancels.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Dialog {
    title: String,
    message: String,
    choices: Vec<(String, Op)>,
    cursor: usize,
    input: Option<String>,
}

impl Dialog {
    fn new(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            choices: Vec::new(),
            cursor: 0,
            input: None,
        }
    }

    fn choice(mut self, label: impl Into<String>, op: Op) -> Self {
        self.choices.push((label.into(), op));
        self
    }
}

/// Something read off the server: waiting for it, what came, or why not.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) enum Load<T> {
    #[default]
    Waiting,
    Ready(T),
    Failed(String),
}

/// What the page shows of this machine, read off the UI thread.
#[derive(Debug, Clone, Default)]
pub(crate) struct Local {
    /// How many tracks each library folder holds.
    folder_tracks: HashMap<PathBuf, u64>,
    server_tracks: u64,
    cache_bytes: u64,
}

/// What the page shows of the signed-in server, read off the UI thread.
#[derive(Debug, Clone, Default)]
pub(crate) struct Remote {
    /// The server these were read from: a sign-in elsewhere drops them.
    url: String,
    profile: Option<ServerProfile>,
    refused: bool,
    api_keys: Load<Vec<KoanApiKey>>,
    app_passwords: Load<Vec<KoanAppPassword>>,
    /// `None` until the list loads, which the server allows an admin alone.
    people: Option<Vec<KoanUser>>,
    scrobbling: Load<Vec<KoanScrobbleService>>,
    assistants: Load<KoanMcp>,
}

impl Remote {
    fn offers(&self, extension: &str) -> bool {
        self.profile.as_ref().is_some_and(|p| p.offers(extension))
    }
}

/// Work finished off the UI thread, applied to the page on the next frame,
/// with a line to say.
type Done = Box<dyn FnOnce(&mut SettingsPage) -> Option<String> + Send>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsHit {
    Section(usize),
    /// The row, anywhere off its controls.
    Row(usize),
    Dec(usize),
    Inc(usize),
    /// The value itself: a choice moves on, a field starts typing, a button
    /// runs.
    Value(usize),
    /// One of a list row's actions, by its place.
    Action(usize, usize),
    DialogChoice(usize),
}

#[derive(Default, Clone)]
struct Geom {
    sections: Rect,
    pane: Rect,
    bar: Rect,
    /// The pane's lines, and the first line of each row.
    total: usize,
    line_of: Vec<usize>,
    links: Vec<(Rect, SettingsHit)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Sections,
    Pane,
}

pub struct SettingsPage {
    cfg: Arc<Config>,
    local: Local,
    remote: Remote,
    connections: Vec<Connection>,
    held: Vec<Held>,
    /// The devices' version the connections were read at.
    devices_seen: u64,
    eq_summary: Option<String>,
    section: Section,
    focus: Focus,
    /// The row the cursor is on, by its key, so it stays on that row as
    /// rows above it come and go.
    cursor: String,
    scroll: usize,
    follow: bool,
    fields: HashMap<Field, String>,
    /// The field being typed into.
    editing: Option<Field>,
    sign_in_with_key: bool,
    new_role: &'static str,
    dialog: Option<Dialog>,
    /// Jobs started and not yet landed.
    busy: usize,
    done_tx: Sender<Done>,
    done_rx: Receiver<Done>,
    geom: Geom,
}

impl Default for SettingsPage {
    fn default() -> Self {
        Self::new()
    }
}

fn inside(r: Rect, col: u16, row: u16) -> bool {
    r.width > 0 && col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

fn signed_in(cfg: &Config) -> bool {
    koan_core::helpers::subsonic_auth(cfg).is_some()
}

impl SettingsPage {
    pub fn new() -> Self {
        let (done_tx, done_rx) = crossbeam_channel::unbounded();
        Self {
            cfg: Config::cached(),
            local: Local::default(),
            remote: Remote::default(),
            connections: Vec::new(),
            held: Vec::new(),
            devices_seen: u64::MAX,
            eq_summary: None,
            section: Section::Library,
            focus: Focus::Sections,
            cursor: String::new(),
            scroll: 0,
            follow: true,
            fields: HashMap::new(),
            editing: None,
            sign_in_with_key: false,
            new_role: ROLES[0].0,
            dialog: None,
            busy: 0,
            done_tx,
            done_rx,
            geom: Geom::default(),
        }
    }

    /// Reads the config again, and what the page shows of the disk and the
    /// server, on threads of their own.
    pub fn reload(&mut self) {
        self.cfg = Config::cached();
        self.follow_devices(true);
        self.load_local();
        self.load_remote();
    }

    /// The page's toolbar: the section in view, and whether work is out.
    pub fn toolbar(&self) -> super::shell::Toolbar {
        let count = match self.busy {
            0 => self.section.label().to_string(),
            _ => format!("{} \u{b7} working\u{2026}", self.section.label()),
        };
        super::shell::Toolbar::new("settings").count(count)
    }

    /// What the EQ section says of the EQ in use.
    pub fn set_eq_summary(&mut self, summary: Option<String>) {
        self.eq_summary = summary;
    }

    /// Whether the page takes every key: a field or a dialog is open.
    pub fn typing(&self) -> bool {
        self.editing.is_some() || self.dialog.is_some()
    }

    /// Takes in finished work, and the connections when the devices moved.
    /// Returns what to say.
    pub fn poll(&mut self) -> Option<String> {
        let mut said = None;
        while let Ok(done) = self.done_rx.try_recv() {
            self.busy = self.busy.saturating_sub(1);
            said = done(self).or(said);
        }
        self.follow_devices(false);
        said
    }

    fn follow_devices(&mut self, force: bool) {
        let version = koan_core::remote::devices::version();
        if !force && version == self.devices_seen {
            return;
        }
        self.devices_seen = version;
        self.connections = koan_core::remote::connections::list();
        self.held = koan_core::remote::connections::held();
    }

    /// Runs `work` on a thread of its own; what it returns is applied on a
    /// later frame.
    fn spawn(&mut self, work: impl FnOnce() -> Done + Send + 'static) {
        let tx = self.done_tx.clone();
        let started = std::thread::Builder::new()
            .name("koan-settings".into())
            .spawn(move || {
                tx.send(work()).ok();
            });
        if started.is_ok() {
            self.busy += 1;
        }
    }

    /// The sections there is something in, in order.
    fn sections(&self) -> Vec<Section> {
        use koan_core::remote::profile::{API_KEYS, MCP, PASSWORDS, SCROBBLING};
        let signed_in = signed_in(&self.cfg);
        let r = &self.remote;
        let mut out = vec![Section::Library, Section::Server];
        if signed_in && (r.offers(PASSWORDS) || r.offers(API_KEYS)) {
            out.push(Section::Account);
        }
        if signed_in && r.people.is_some() {
            out.push(Section::People);
        }
        out.extend([Section::Playback, Section::Eq, Section::Devices]);
        if signed_in && (r.offers(SCROBBLING) || r.offers(MCP)) {
            out.push(Section::Integrations);
        }
        out.push(Section::Terminal);
        out
    }

    fn rows(&self) -> Vec<Row> {
        rows::build(self)
    }

    fn field(&self, field: Field) -> &str {
        self.fields.get(&field).map_or("", String::as_str)
    }

    fn take_field(&mut self, field: Field) -> String {
        self.fields
            .remove(&field)
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    /// The row the cursor is on, or the first that can hold it.
    fn cursor_at(&self, rows: &[Row]) -> Option<usize> {
        rows.iter()
            .position(|r| r.key == self.cursor && r.kind.selectable())
            .or_else(|| rows.iter().position(|r| r.kind.selectable()))
    }

    fn select_section(&mut self, section: Section) {
        if self.section != section {
            self.section = section;
            self.cursor.clear();
            self.scroll = 0;
            self.follow = true;
        }
    }

    fn step_section(&mut self, delta: isize) {
        let sections = self.sections();
        let at = sections
            .iter()
            .position(|s| *s == self.section)
            .unwrap_or(0);
        let next = (at as isize + delta).clamp(0, sections.len() as isize - 1) as usize;
        self.select_section(sections[next]);
    }

    fn move_cursor(&mut self, delta: isize) {
        let rows = self.rows();
        let selectable: Vec<usize> = (0..rows.len())
            .filter(|&i| rows[i].kind.selectable())
            .collect();
        if selectable.is_empty() {
            return;
        }
        let at = self
            .cursor_at(&rows)
            .and_then(|c| selectable.iter().position(|&i| i == c))
            .unwrap_or(0);
        let next = (at as isize + delta).clamp(0, selectable.len() as isize - 1) as usize;
        self.cursor = rows[selectable[next]].key.clone();
        self.follow = true;
    }

    pub fn scroll_by(&mut self, rows: isize) {
        self.follow = false;
        let max = self
            .geom
            .total
            .saturating_sub(self.geom.pane.height as usize);
        self.scroll = self.scroll.saturating_add_signed(rows).min(max);
    }

    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Option<Option<Outcome>> {
        use crossterm::event::{KeyCode, KeyModifiers};
        if self.dialog.is_some() {
            return Some(self.dialog_key(key));
        }
        if let Some(field) = self.editing {
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            let text = self.fields.entry(field).or_default();
            match key.code {
                KeyCode::Esc => self.editing = None,
                KeyCode::Enter => {
                    self.editing = None;
                    let submit = self.rows().into_iter().find_map(|r| match r.kind {
                        Kind::Field {
                            field: f, submit, ..
                        } if f == field => submit,
                        _ => None,
                    });
                    return Some(submit.and_then(|op| self.run(op)));
                }
                KeyCode::Backspace => {
                    text.pop();
                }
                KeyCode::Char('u') if ctrl => text.clear(),
                KeyCode::Char(c) if !ctrl => text.push(c),
                _ => {}
            }
            return Some(None);
        }
        if self.focus == Focus::Sections {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.step_section(-1),
                KeyCode::Down | KeyCode::Char('j') => self.step_section(1),
                KeyCode::Home | KeyCode::Char('g') => self.step_section(-100),
                KeyCode::End | KeyCode::Char('G') => self.step_section(100),
                KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter | KeyCode::Char(' ') => {
                    self.focus = Focus::Pane;
                    self.follow = true;
                }
                _ => return None,
            }
            return Some(None);
        }
        let rows = self.rows();
        let at = self.cursor_at(&rows);
        let kind = at.map(|i| rows[i].kind.clone());
        let page = (self.geom.pane.height as isize).max(1);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_cursor(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_cursor(1),
            KeyCode::PageUp => self.move_cursor(-page),
            KeyCode::PageDown => self.move_cursor(page),
            KeyCode::Home | KeyCode::Char('g') => self.move_cursor(-10_000),
            KeyCode::End | KeyCode::Char('G') => self.move_cursor(10_000),
            KeyCode::Esc => self.focus = Focus::Sections,
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('-') => match kind {
                Some(Kind::Choice { step, .. }) => return Some(self.step(step, false)),
                _ if key.code != KeyCode::Char('-') => self.focus = Focus::Sections,
                _ => {}
            },
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Char('+') | KeyCode::Char('=') => {
                if let Some(Kind::Choice { step, .. }) = kind {
                    return Some(self.step(step, true));
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                return Some(kind.and_then(|k| self.activate(k)));
            }
            KeyCode::Char('m') => {
                if let Some(Kind::Item { title, actions, .. }) = kind
                    && !actions.is_empty()
                {
                    return Some(self.run(Op::Menu(title, actions)));
                }
            }
            KeyCode::Char('x') | KeyCode::Delete => {
                return match kind {
                    Some(Kind::Item {
                        remove: Some(op), ..
                    }) => Some(self.run(op)),
                    _ => None,
                };
            }
            _ => return None,
        }
        Some(None)
    }

    fn dialog_key(&mut self, key: crossterm::event::KeyEvent) -> Option<Outcome> {
        use crossterm::event::KeyCode;
        let dialog = self.dialog.as_mut()?;
        let n = dialog.choices.len().max(1);
        let typing = dialog.input.is_some();
        match key.code {
            KeyCode::Esc => self.dialog = None,
            KeyCode::Left | KeyCode::Up | KeyCode::BackTab => {
                dialog.cursor = (dialog.cursor + n - 1) % n
            }
            KeyCode::Right | KeyCode::Down | KeyCode::Tab => {
                dialog.cursor = (dialog.cursor + 1) % n
            }
            KeyCode::Char('h' | 'k') if !typing => dialog.cursor = (dialog.cursor + n - 1) % n,
            KeyCode::Char('l' | 'j') if !typing => dialog.cursor = (dialog.cursor + 1) % n,
            KeyCode::Enter => return self.choose(None),
            KeyCode::Backspace => {
                if let Some(input) = dialog.input.as_mut() {
                    input.pop();
                }
            }
            KeyCode::Char(c) => {
                if let Some(input) = dialog.input.as_mut() {
                    input.push(c);
                }
            }
            _ => {}
        }
        None
    }

    /// Runs the dialog's choice at `idx`, or the one under its cursor.
    fn choose(&mut self, idx: Option<usize>) -> Option<Outcome> {
        let dialog = self.dialog.take()?;
        let (_, op) = dialog.choices.get(idx.unwrap_or(dialog.cursor))?.clone();
        match op {
            Op::SetPassword(username) => {
                let password = dialog.input.unwrap_or_default();
                self.set_password(username, password)
            }
            op => self.run(op),
        }
    }

    /// What `Enter`, or a click on its value, does to a row.
    fn activate(&mut self, kind: Kind) -> Option<Outcome> {
        match kind {
            Kind::Choice { step, .. } => self.step(step, true),
            Kind::Field { field, .. } => {
                self.editing = Some(field);
                None
            }
            Kind::Button { op, .. } | Kind::Info { op: Some(op), .. } => self.run(op),
            Kind::Item { actions, title, .. } => match actions.as_slice() {
                [] => None,
                [(_, op)] => self.run(op.clone()),
                _ => self.run(Op::Menu(title, actions)),
            },
            _ => None,
        }
    }

    pub fn hit(&self, col: u16, row: u16) -> Option<SettingsHit> {
        let g = &self.geom;
        if let Some((_, hit)) = g.links.iter().rev().find(|(r, _)| inside(*r, col, row)) {
            return Some(*hit);
        }
        if self.dialog.is_some() {
            return None;
        }
        if inside(g.sections, col, row) {
            return Some(SettingsHit::Section((row - g.sections.y) as usize));
        }
        if inside(g.pane, col, row) {
            let line = self.scroll + (row - g.pane.y) as usize;
            let idx = g.line_of.iter().position(|&l| l == line)?;
            if self.rows().get(idx).is_some_and(|r| r.kind.selectable()) {
                return Some(SettingsHit::Row(idx));
            }
        }
        None
    }

    pub fn click(&mut self, hit: SettingsHit) -> Option<Outcome> {
        if self.dialog.is_some() {
            return match hit {
                SettingsHit::DialogChoice(i) => self.choose(Some(i)),
                _ => None,
            };
        }
        self.editing = None;
        let idx = match hit {
            SettingsHit::Section(i) => {
                if let Some(s) = self.sections().get(i) {
                    self.select_section(*s);
                }
                self.focus = Focus::Sections;
                return None;
            }
            SettingsHit::DialogChoice(_) => return None,
            SettingsHit::Row(i)
            | SettingsHit::Dec(i)
            | SettingsHit::Inc(i)
            | SettingsHit::Value(i)
            | SettingsHit::Action(i, _) => i,
        };
        let row = self.rows().into_iter().nth(idx)?;
        self.cursor = row.key.clone();
        self.focus = Focus::Pane;
        self.follow = true;
        match (hit, row.kind) {
            (SettingsHit::Dec(_), Kind::Choice { step, .. }) => self.step(step, false),
            (SettingsHit::Inc(_), Kind::Choice { step, .. }) => self.step(step, true),
            (SettingsHit::Value(_), kind) => self.activate(kind),
            (SettingsHit::Action(_, a), Kind::Item { actions, .. }) => {
                let op = actions.into_iter().nth(a)?.1;
                self.run(op)
            }
            _ => None,
        }
    }

    pub fn on_scrollbar(&self, col: u16, row: u16) -> bool {
        inside(self.geom.bar, col, row)
    }

    pub fn scroll_to_bar(&mut self, row: u16) {
        self.follow = false;
        let b = self.geom.bar;
        if b.height == 0 {
            return;
        }
        let max = self
            .geom
            .total
            .saturating_sub(self.geom.pane.height as usize);
        let frac = row.saturating_sub(b.y) as f64 / b.height.saturating_sub(1).max(1) as f64;
        self.scroll = ((frac.clamp(0.0, 1.0) * max as f64).round() as usize).min(max);
    }

    /// Writes a change to the config and reads it back. The device settings
    /// reach the network at once.
    fn persist(&mut self, change: impl FnOnce(&mut Config)) -> Result<(), String> {
        let wrote = Config::persist(change).map_err(|e| format!("could not save: {e}"));
        self.cfg = Config::cached();
        koan_core::remote::nearby::reconfigure();
        wrote
    }

    /// Moves a choice one step, `forward` or back.
    fn step(&mut self, step: Step, forward: bool) -> Option<Outcome> {
        match step {
            Step::Setting(Setting::Offline) => {
                koan_core::remote::offline::set_manual(!koan_core::remote::offline::manual());
                Some(Outcome::Changed(Setting::Offline))
            }
            Step::Setting(setting) => Some(match self.persist(|c| setting.step(c, forward)) {
                Ok(()) => Outcome::Changed(setting),
                Err(e) => Outcome::Say(e),
            }),
            Step::SignInWith => {
                self.sign_in_with_key = !self.sign_in_with_key;
                None
            }
            Step::NewRole => {
                self.new_role = cycle_role(self.new_role, forward);
                None
            }
        }
    }
}

const REPLAYGAIN: [&str; 3] = ["off", "track", "album"];
const THEMES: [&str; 2] = ["koan", "ansi"];
const PALETTES: [&str; 5] = ["theme", "rainbow", "mono", "fire", "neon"];
const FRAME_RATES: [u8; 3] = [30, 60, 120];
const SYNC_MINUTES: [u64; 5] = [0, 15, 60, 360, 1440];

/// The palette's name as the page lists it: `spectrum` and `record` are older
/// names for the themed palette.
fn palette_name(configured: &str) -> &str {
    match configured {
        "spectrum" | "record" | "" => "theme",
        other => other,
    }
}

fn on_off(on: bool) -> String {
    if on { "on" } else { "off" }.into()
}

/// The name after (or before) `now` in `names`, round the end.
fn cycle<'a>(names: &[&'a str], now: &str, forward: bool) -> &'a str {
    let at = names.iter().position(|n| *n == now).unwrap_or(0);
    let n = names.len();
    names[if forward {
        (at + 1) % n
    } else {
        (at + n - 1) % n
    }]
}

fn replaygain_name(mode: ReplayGainMode) -> &'static str {
    match mode {
        ReplayGainMode::Off => "off",
        ReplayGainMode::Track => "track",
        ReplayGainMode::Album => "album",
    }
}

impl Setting {
    fn label(self) -> &'static str {
        match self {
            Setting::ReplayGain => "replaygain",
            Setting::PreAmp => "pre-amp",
            Setting::FadeOnPause => "fade on pause",
            Setting::Theme => "theme",
            Setting::CoverGraphics => "cover graphics",
            Setting::ArtSize => "cover size",
            Setting::ShowFps => "frame counter",
            Setting::FrameRate => "frame rate",
            Setting::Palette => "visualiser colours",
            Setting::AutoSync => "keep in sync",
            Setting::SyncInterval => "every",
            Setting::DownloadWorkers => "parallel downloads",
            Setting::Offline => "offline mode",
            Setting::Discoverable => "discoverable",
            Setting::NearbyControl => "devices on network",
        }
    }

    /// A few words on what it does, for the right of the row.
    fn note(self) -> &'static str {
        match self {
            Setting::ReplayGain => "the gain written into the file's tags",
            Setting::PreAmp => "on top of the gain",
            Setting::FadeOnPause => "ramp the volume instead of cutting",
            Setting::Theme => "kōan's colours or the terminal's own",
            Setting::CoverGraphics => "images where the terminal draws them",
            Setting::ArtSize => "columns, in the transport bar",
            Setting::ShowFps => "top right",
            Setting::FrameRate => "from the next start",
            Setting::Palette => "theme follows the cover",
            Setting::AutoSync => "in the apps",
            Setting::SyncInterval => "",
            Setting::DownloadWorkers => "from the next start",
            Setting::Offline => "only what is on this device",
            Setting::Discoverable => "other kōan apps on this network see it",
            Setting::NearbyControl => "what they may do here",
        }
    }

    fn value(self, cfg: &Config) -> String {
        match self {
            Setting::ReplayGain => match cfg.playback.replaygain {
                ReplayGainMode::Off => "off",
                ReplayGainMode::Track => "per track",
                ReplayGainMode::Album => "per album",
            }
            .into(),
            Setting::PreAmp => format!("{:+.1} dB", cfg.playback.pre_amp_db),
            Setting::FadeOnPause => on_off(cfg.playback.fade_on_pause),
            Setting::Theme => cfg.playback.tui_theme.clone(),
            Setting::CoverGraphics => on_off(cfg.playback.art_graphics),
            Setting::ArtSize => cfg.playback.art_size.to_string(),
            Setting::ShowFps => on_off(cfg.playback.show_fps),
            Setting::FrameRate => format!("{} fps", cfg.playback.target_fps),
            Setting::Palette => palette_name(&cfg.visualizer.palette).into(),
            Setting::AutoSync => on_off(cfg.remote.auto_sync),
            Setting::SyncInterval => match cfg.remote.auto_sync_interval_mins {
                0 => "startup only".into(),
                60 => "hour".into(),
                1440 => "day".into(),
                n if n % 60 == 0 => format!("{} hours", n / 60),
                n => format!("{n} minutes"),
            },
            Setting::DownloadWorkers => cfg.remote.download_workers.to_string(),
            Setting::Offline => on_off(koan_core::remote::offline::manual()),
            Setting::Discoverable => on_off(cfg.devices.discoverable),
            Setting::NearbyControl => match cfg.devices.nearby_control {
                NearbyControl::Full => "full control",
                NearbyControl::Playback => "playback only",
            }
            .into(),
        }
    }

    /// Moves the setting one step in `c`, `forward` or back.
    fn step(self, c: &mut Config, forward: bool) {
        let d: isize = if forward { 1 } else { -1 };
        match self {
            Setting::ReplayGain => {
                let now = replaygain_name(c.playback.replaygain);
                c.playback.replaygain = match cycle(&REPLAYGAIN, now, forward) {
                    "track" => ReplayGainMode::Track,
                    "album" => ReplayGainMode::Album,
                    _ => ReplayGainMode::Off,
                };
            }
            Setting::PreAmp => {
                c.playback.pre_amp_db = (c.playback.pre_amp_db + 0.5 * d as f64).clamp(-15.0, 15.0);
            }
            Setting::FadeOnPause => c.playback.fade_on_pause = !c.playback.fade_on_pause,
            Setting::Theme => {
                c.playback.tui_theme = cycle(&THEMES, &c.playback.tui_theme, forward).into();
            }
            Setting::CoverGraphics => c.playback.art_graphics = !c.playback.art_graphics,
            Setting::ArtSize => {
                c.playback.art_size = (c.playback.art_size as isize + 2 * d).clamp(4, 80) as u16;
            }
            Setting::ShowFps => c.playback.show_fps = !c.playback.show_fps,
            Setting::FrameRate => {
                let at = FRAME_RATES
                    .iter()
                    .position(|f| *f == c.playback.target_fps)
                    .unwrap_or(1) as isize;
                c.playback.target_fps =
                    FRAME_RATES[(at + d).rem_euclid(FRAME_RATES.len() as isize) as usize];
            }
            Setting::Palette => {
                let now = palette_name(&c.visualizer.palette).to_string();
                c.visualizer.palette = cycle(&PALETTES, &now, forward).into();
            }
            Setting::AutoSync => c.remote.auto_sync = !c.remote.auto_sync,
            Setting::SyncInterval => {
                let at = SYNC_MINUTES
                    .iter()
                    .position(|m| *m >= c.remote.auto_sync_interval_mins)
                    .unwrap_or(0) as isize;
                c.remote.auto_sync_interval_mins =
                    SYNC_MINUTES[(at + d).rem_euclid(SYNC_MINUTES.len() as isize) as usize];
            }
            Setting::DownloadWorkers => {
                c.remote.download_workers =
                    (c.remote.download_workers as isize + d).clamp(1, 16) as usize;
            }
            Setting::Offline => {}
            Setting::Discoverable => c.devices.discoverable = !c.devices.discoverable,
            Setting::NearbyControl => {
                c.devices.nearby_control = match c.devices.nearby_control {
                    NearbyControl::Full => NearbyControl::Playback,
                    NearbyControl::Playback => NearbyControl::Full,
                };
            }
        }
    }
}

/// The cells the section list takes, its rule included.
const SECTIONS_W: u16 = 17;
/// Where values start in the pane, after the labels.
const LABEL_W: u16 = 26;

impl SettingsPage {
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        hover: Option<SettingsHit>,
    ) {
        self.geom = Geom::default();
        if area.width < 40 || area.height < 2 {
            return;
        }
        let body = area;
        let sections = self.sections();
        if !sections.contains(&self.section) {
            self.section = Section::Library;
        }

        let list = Rect::new(body.x, body.y, SECTIONS_W - 2, body.height);
        self.geom.sections = Rect::new(
            list.x,
            list.y,
            list.width,
            (sections.len() as u16).min(list.height),
        );
        for (i, s) in sections.iter().enumerate().take(list.height as usize) {
            let y = list.y + i as u16;
            let chosen = *s == self.section;
            let style = match (chosen, self.focus == Focus::Sections) {
                (true, true) => theme.library_cursor,
                (true, false) => theme.heading.add_modifier(Modifier::BOLD),
                _ if hover == Some(SettingsHit::Section(i)) => theme.library_hover,
                _ => theme.hint_desc,
            };
            for cx in list.x..list.x + list.width {
                buf[(cx, y)].set_style(style);
            }
            if chosen {
                buf[(list.x, y)].set_char('\u{258c}').set_style(theme.bar);
            }
            buf.set_stringn(list.x + 2, y, s.label(), list.width as usize - 2, style);
        }
        let rule_x = body.x + SECTIONS_W - 1;
        for y in body.y..body.y + body.height {
            buf[(rule_x, y)].set_char('\u{2502}').set_style(theme.rule);
        }

        let pane = Rect::new(
            rule_x + 2,
            body.y,
            (body.x + body.width).saturating_sub(rule_x + 2),
            body.height,
        );
        self.render_pane(pane, buf, theme, hover);
        if self.dialog.is_some() {
            self.render_dialog(area, buf, theme, hover);
        }
    }

    fn render_pane(
        &mut self,
        pane: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        hover: Option<SettingsHit>,
    ) {
        self.geom.pane = pane;
        let rows = self.rows();
        let text_w = pane.width.saturating_sub(4) as usize;
        // Each row as the lines it takes: a note wraps, and a heading has a
        // blank line above it but at the top.
        let mut lines: Vec<(usize, Option<String>)> = Vec::new();
        let mut line_of = Vec::with_capacity(rows.len());
        for (i, row) in rows.iter().enumerate() {
            if matches!(row.kind, Kind::Heading(_)) && i > 0 {
                lines.push((i, None));
            }
            line_of.push(lines.len());
            match &row.kind {
                Kind::Note(text) | Kind::Warn(text) => {
                    lines.extend(wrap(text, text_w).into_iter().map(|l| (i, Some(l))));
                }
                _ => lines.push((i, None)),
            }
        }
        let height = pane.height as usize;
        let cursor = (self.focus == Focus::Pane)
            .then(|| self.cursor_at(&rows))
            .flatten();
        if let Some(c) = cursor {
            self.cursor = rows[c].key.clone();
            if self.follow {
                let line = line_of[c];
                // The heading over a group's first row stays with it.
                let top = (0..c)
                    .rev()
                    .take_while(|&j| !rows[j].kind.selectable())
                    .find(|&j| matches!(rows[j].kind, Kind::Heading(_)))
                    .map(|j| line_of[j])
                    .filter(|&l| line - l < height)
                    .unwrap_or(line);
                if top < self.scroll {
                    self.scroll = top;
                } else if line >= self.scroll + height {
                    self.scroll = line + 1 - height;
                }
            }
        }
        self.scroll = self.scroll.min(lines.len().saturating_sub(height));
        let bar = lines.len() > height;
        let end = pane.x + pane.width - if bar { 3 } else { 0 };

        for r in 0..height {
            let Some((idx, text)) = lines.get(self.scroll + r) else {
                break;
            };
            let y = pane.y + r as u16;
            let row = &rows[*idx];
            let first = line_of[*idx] == self.scroll + r;
            let room = (end - pane.x) as usize;
            match (&row.kind, text) {
                (Kind::Note(_), Some(t)) => {
                    buf.set_stringn(pane.x + 2, y, t, room, theme.hint_desc);
                }
                (Kind::Warn(_), Some(t)) => {
                    let style = Style::new().fg(theme.tokens.bad);
                    buf.set_stringn(pane.x + 2, y, t, room, style);
                }
                (Kind::Heading(name), None) if first => {
                    let style = theme.heading.add_modifier(Modifier::BOLD);
                    buf.set_stringn(pane.x, y, name, room, style);
                    let from = pane.x + 1 + name.width() as u16;
                    for cx in from..end {
                        buf[(cx, y)].set_char('\u{2500}').set_style(theme.rule);
                    }
                }
                (_, None) if first => {
                    let line = Rect::new(pane.x, y, end - pane.x, 1);
                    self.render_row(buf, theme, *idx, row, line, hover);
                }
                _ => {}
            }
        }
        self.geom.total = lines.len();
        self.geom.line_of = line_of;

        if bar {
            let x = pane.x + pane.width - 1;
            self.geom.bar = Rect::new(x - 2, pane.y, 3, pane.height);
            let total = lines.len();
            let thumb = (height * height / total).max(1);
            let top = (height - thumb) * self.scroll / (total - height).max(1);
            for row in 0..height {
                let on = row >= top && row < top + thumb;
                for cx in [x - 1, x] {
                    buf[(cx, pane.y + row as u16)]
                        .set_char(if on { '\u{2588}' } else { ' ' })
                        .set_style(if on {
                            Style::new().fg(theme.scrollbar_thumb)
                        } else {
                            Style::new().bg(theme.scrollbar_bg)
                        });
                }
            }
        }
    }

    /// Draws a row that takes one line: its label, then its value or its
    /// controls from the value column.
    fn render_row(
        &mut self,
        buf: &mut Buffer,
        theme: &Theme,
        idx: usize,
        row: &Row,
        line: Rect,
        hover: Option<SettingsHit>,
    ) {
        let (x0, y, end) = (line.x, line.y, line.x + line.width);
        // On a narrow pane the labels give up room before the values do.
        let value_x = x0 + LABEL_W.min(line.width * 2 / 5);
        let selected = self.focus == Focus::Pane && row.key == self.cursor;
        let hovered = matches!(hover,
            Some(SettingsHit::Row(n) | SettingsHit::Dec(n) | SettingsHit::Inc(n)
                | SettingsHit::Value(n) | SettingsHit::Action(n, _)) if n == idx);
        if selected {
            for cx in x0..end {
                buf[(cx, y)].set_style(theme.library_cursor);
            }
            buf[(x0, y)].set_char('\u{258c}').set_style(theme.bar);
        } else if hovered {
            for cx in x0..end {
                buf[(cx, y)].set_style(theme.library_hover);
            }
        }
        let (base, dim) = if selected {
            (theme.library_cursor, theme.library_cursor)
        } else {
            (theme.track_normal, theme.hint_desc)
        };
        let label_w = (value_x - x0).saturating_sub(4) as usize;
        let room = |x: u16| end.saturating_sub(x) as usize;
        let lit = |on: bool, s: Style| {
            if on {
                s.add_modifier(Modifier::UNDERLINED)
            } else {
                s
            }
        };
        let on = |hit: SettingsHit| hover == Some(hit);
        let mut links = Vec::new();
        match &row.kind {
            Kind::Info { label, value, op } => {
                buf.set_stringn(x0 + 2, y, fit(label, label_w), label_w, dim);
                let shown = fit(value, room(value_x));
                let style = lit(op.is_some() && on(SettingsHit::Value(idx)), base);
                buf.set_stringn(value_x, y, &shown, room(value_x), style);
                if op.is_some() {
                    links.push((
                        Rect::new(value_x, y, shown.width() as u16, 1),
                        SettingsHit::Value(idx),
                    ));
                }
            }
            Kind::Choice {
                label, value, note, ..
            } => {
                buf.set_stringn(x0 + 2, y, fit(label, label_w), label_w, base);
                let vw = value.width() as u16;
                if value_x + vw + 4 > end {
                    return;
                }
                let arrow = |hit| {
                    if on(hit) {
                        dim.add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                    } else {
                        dim
                    }
                };
                let value_style = lit(
                    on(SettingsHit::Value(idx)),
                    base.add_modifier(Modifier::BOLD),
                );
                buf.set_string(value_x, y, "\u{2039}", arrow(SettingsHit::Dec(idx)));
                buf.set_string(value_x + 2, y, value, value_style);
                buf.set_string(
                    value_x + 3 + vw,
                    y,
                    "\u{203a}",
                    arrow(SettingsHit::Inc(idx)),
                );
                links.push((Rect::new(value_x, y, 2, 1), SettingsHit::Dec(idx)));
                links.push((
                    Rect::new(value_x + 2, y, vw + 1, 1),
                    SettingsHit::Value(idx),
                ));
                links.push((Rect::new(value_x + 3 + vw, y, 2, 1), SettingsHit::Inc(idx)));
                // A note is shown whole or not at all: cut, it says nothing.
                let note_x = value_x + vw + 7;
                if !note.is_empty() && note.width() <= room(note_x) {
                    buf.set_string(note_x, y, note, dim);
                }
            }
            Kind::Field { field, label, .. } => {
                buf.set_stringn(x0 + 2, y, fit(label, label_w), label_w, base);
                let typed = self.field(*field);
                let editing = self.editing == Some(*field);
                let w = room(value_x).saturating_sub(1);
                let shown = if field.secret() {
                    "\u{2022}".repeat(typed.chars().count())
                } else {
                    typed.to_string()
                };
                let (text, style) = if shown.is_empty() && !editing {
                    (
                        fit(field.placeholder(), w),
                        dim.add_modifier(Modifier::ITALIC),
                    )
                } else if editing {
                    (
                        tail(&shown, w.saturating_sub(1)),
                        theme.hint_key.add_modifier(Modifier::UNDERLINED),
                    )
                } else {
                    (fit(&shown, w), lit(on(SettingsHit::Value(idx)), base))
                };
                buf.set_stringn(value_x, y, &text, w, style);
                if editing {
                    let cx = value_x + text.width() as u16;
                    if cx < end {
                        buf[(cx, y)].set_char('\u{2588}').set_style(theme.hint_key);
                    }
                }
                links.push((Rect::new(value_x, y, w as u16, 1), SettingsHit::Value(idx)));
            }
            Kind::Button { label, note, .. } => {
                let shown = format!("[ {label} ]");
                let style = lit(on(SettingsHit::Value(idx)), theme.track_playing);
                let style = if selected {
                    style.add_modifier(Modifier::BOLD)
                } else {
                    style
                };
                buf.set_stringn(x0 + 2, y, &shown, room(x0 + 2), style);
                links.push((
                    Rect::new(x0 + 2, y, (shown.width() as u16).min(end - x0 - 2), 1),
                    SettingsHit::Value(idx),
                ));
                let note_x = (x0 + 4 + shown.width() as u16).max(value_x);
                if !note.is_empty() && note.width() <= room(note_x) {
                    buf.set_string(note_x, y, note, dim);
                }
            }
            Kind::Item {
                title,
                detail,
                actions,
                ..
            } => {
                // The actions at the row's end, laid out from the right.
                let mut x = end;
                let mut placed = Vec::new();
                for (a, (name, _)) in actions.iter().enumerate().rev() {
                    let w = name.width() as u16 + 2;
                    if x < value_x + w {
                        break;
                    }
                    x -= w;
                    placed.push((x, a, *name));
                }
                for (ax, a, name) in placed {
                    let hit = SettingsHit::Action(idx, a);
                    let style = if on(hit) {
                        theme
                            .heading
                            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                    } else {
                        dim
                    };
                    buf.set_string(ax + 1, y, name, style);
                    links.push((Rect::new(ax, y, name.width() as u16 + 2, 1), hit));
                }
                let title_w = (x.saturating_sub(x0 + 2) as usize).saturating_sub(2);
                let shown = fit(title, title_w);
                buf.set_stringn(x0 + 2, y, &shown, title_w, base);
                let dx = (x0 + 4 + shown.width() as u16).max(value_x);
                if !detail.is_empty() && dx + 4 < x {
                    let w = (x - dx).saturating_sub(1) as usize;
                    buf.set_stringn(dx, y, fit(detail, w), w, dim);
                }
            }
            Kind::Heading(_) | Kind::Note(_) | Kind::Warn(_) => {}
        }
        self.geom.links.extend(links);
    }

    fn render_dialog(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        hover: Option<SettingsHit>,
    ) {
        let Some(d) = self.dialog.as_ref() else {
            return;
        };
        let w = 64u16.min(area.width.saturating_sub(4));
        let inner_w = w.saturating_sub(4) as usize;
        let message = wrap(&d.message, inner_w);
        let input = if d.input.is_some() { 2 } else { 0 };
        let h = ((message.len() + input + 5) as u16).min(area.height);
        let popup = Rect::new(
            area.x + (area.width - w) / 2,
            area.y + (area.height - h) / 2,
            w,
            h,
        );
        crate::shell::clear(buf, popup);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(theme.rule)
            .title(ratatui::text::Span::styled(
                format!(" {} ", fit(&d.title, inner_w)),
                theme.heading.add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(popup);
        block.render(popup, buf);
        if inner.height < 2 {
            return;
        }
        let x = inner.x + 1;
        let bottom = inner.y + inner.height;
        let mut y = inner.y + 1;
        for line in &message {
            if y + 1 >= bottom {
                break;
            }
            buf.set_stringn(x, y, line, inner_w, theme.track_normal);
            y += 1;
        }
        if let Some(typed) = &d.input
            && y + 2 < bottom
        {
            y += 1;
            let dots = fit(&"\u{2022}".repeat(typed.chars().count()), inner_w - 1);
            buf.set_stringn(x, y, &dots, inner_w, theme.hint_key);
            buf[(x + dots.width() as u16, y)]
                .set_char('\u{2588}')
                .set_style(theme.hint_key);
        }
        // The choices along the foot, the one under the cursor lit.
        let y = bottom - 1;
        let mut cx = x;
        let mut links = Vec::new();
        for (i, (label, _)) in d.choices.iter().enumerate() {
            let shown = format!(" {label} ");
            let cells = shown.width() as u16;
            if cx + cells > inner.x + inner.width {
                break;
            }
            let style = if i == d.cursor {
                theme.picker_cursor
            } else if hover == Some(SettingsHit::DialogChoice(i)) {
                theme.library_hover
            } else {
                theme.hint_key
            };
            buf.set_string(cx, y, &shown, style);
            links.push((Rect::new(cx, y, cells, 1), SettingsHit::DialogChoice(i)));
            cx += cells + 1;
        }
        self.geom.links.extend(links);
    }
}

#[cfg(test)]
mod tests;
