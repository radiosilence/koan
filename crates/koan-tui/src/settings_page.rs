//! The Settings page: the settings that otherwise mean editing
//! `config.toml`. Each change is written through `Config::persist`, which
//! puts it in the file that owns it, as the apps' Settings does.

use std::sync::Arc;

use koan_core::config::{Config, NearbyControl, ReplayGainMode};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::theme::Theme;

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
    Discoverable,
    NearbyControl,
    Offline,
}

/// Work a row starts rather than a value it sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Task {
    Scan,
    Sync,
}

impl Task {
    fn label(self) -> &'static str {
        match self {
            Task::Scan => "scan library",
            Task::Sync => "sync now",
        }
    }

    fn note(self) -> &'static str {
        match self {
            Task::Scan => "read the library folders for new, changed and removed files",
            Task::Sync => "pull the server's library, favourites and playlists",
        }
    }
}

/// What a change on the page asks of the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Changed(Setting),
    Run(Task),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsHit {
    /// The row, anywhere off its controls.
    Row(usize),
    Dec(usize),
    Inc(usize),
    /// The value itself: a toggle flips, a choice moves on.
    Value(usize),
}

enum Entry {
    Section(&'static str),
    Item(Setting),
    Action(Task),
    /// Shown, not edited here: what the apps set through folders and sign-in.
    Info(&'static str, String),
}

const REPLAYGAIN: [&str; 3] = ["off", "track", "album"];
const THEMES: [&str; 2] = ["koan", "ansi"];
const PALETTES: [&str; 5] = ["theme", "rainbow", "mono", "fire", "neon"];
const FRAME_RATES: [u8; 3] = [30, 60, 120];
const NEARBY: [&str; 2] = ["full", "playback"];

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
            Setting::AutoSync => "sync the server",
            Setting::Discoverable => "visible to devices",
            Setting::NearbyControl => "devices may",
            Setting::Offline => "offline mode",
        }
    }

    /// A line on why, for the right of the row.
    fn note(self) -> &'static str {
        match self {
            Setting::ReplayGain => "level tracks or albums alike",
            Setting::PreAmp => "dB on top of the gain",
            Setting::FadeOnPause => "instead of cutting",
            Setting::Theme => "koan's colours or the terminal's own",
            Setting::CoverGraphics => "images where the terminal draws them",
            Setting::ArtSize => "columns, in the transport bar",
            Setting::ShowFps => "top right",
            Setting::FrameRate => "applies at the next start",
            Setting::Palette => "theme follows the cover",
            Setting::AutoSync => "at start and on a timer",
            Setting::Discoverable => "announce this device on the network",
            Setting::NearbyControl => "full, or the queue and playback only",
            Setting::Offline => "play only what is on this device",
        }
    }

    fn value(self, cfg: &Config) -> String {
        match self {
            Setting::ReplayGain => match cfg.playback.replaygain {
                ReplayGainMode::Off => "off",
                ReplayGainMode::Track => "track",
                ReplayGainMode::Album => "album",
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
            Setting::Discoverable => on_off(cfg.devices.discoverable),
            Setting::NearbyControl => match cfg.devices.nearby_control {
                NearbyControl::Full => "full",
                NearbyControl::Playback => "playback",
            }
            .into(),
            Setting::Offline => on_off(koan_core::remote::offline::manual()),
        }
    }

    /// Moves the setting one step, `forward` or back, and writes it.
    fn step(self, forward: bool) -> bool {
        if self == Setting::Offline {
            koan_core::remote::offline::set_manual(!koan_core::remote::offline::manual());
            return true;
        }
        let d: isize = if forward { 1 } else { -1 };
        let cycle = |names: &[&str], now: &str| -> String {
            let at = names.iter().position(|n| *n == now).unwrap_or(0) as isize;
            names[(at + d).rem_euclid(names.len() as isize) as usize].to_string()
        };
        Config::persist(|c| match self {
            Setting::ReplayGain => {
                let now = match c.playback.replaygain {
                    ReplayGainMode::Off => "off",
                    ReplayGainMode::Track => "track",
                    ReplayGainMode::Album => "album",
                };
                c.playback.replaygain = match cycle(&REPLAYGAIN, now).as_str() {
                    "track" => ReplayGainMode::Track,
                    "album" => ReplayGainMode::Album,
                    _ => ReplayGainMode::Off,
                };
            }
            Setting::PreAmp => {
                c.playback.pre_amp_db = (c.playback.pre_amp_db + 0.5 * d as f64).clamp(-12.0, 12.0);
            }
            Setting::FadeOnPause => c.playback.fade_on_pause = !c.playback.fade_on_pause,
            Setting::Theme => c.playback.tui_theme = cycle(&THEMES, &c.playback.tui_theme),
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
                c.visualizer.palette = cycle(&PALETTES, palette_name(&c.visualizer.palette));
            }
            Setting::AutoSync => c.remote.auto_sync = !c.remote.auto_sync,
            Setting::Discoverable => c.devices.discoverable = !c.devices.discoverable,
            Setting::Offline => {}
            Setting::NearbyControl => {
                let now = match c.devices.nearby_control {
                    NearbyControl::Full => "full",
                    NearbyControl::Playback => "playback",
                };
                c.devices.nearby_control = match cycle(&NEARBY, now).as_str() {
                    "playback" => NearbyControl::Playback,
                    _ => NearbyControl::Full,
                };
            }
        })
        .is_ok()
    }
}

/// The palette's name as the page lists it: `spectrum` and `record` are older
/// names for the themed palette.
fn palette_name(configured: &str) -> &str {
    match configured {
        "spectrum" | "record" | "" => "theme",
        other => other,
    }
}

/// Whether the cursor can rest on `entry`.
fn selectable(entry: &Entry) -> bool {
    matches!(entry, Entry::Item(_) | Entry::Action(_))
}

fn on_off(on: bool) -> String {
    if on { "on" } else { "off" }.into()
}

#[derive(Default, Clone)]
struct Geom {
    list: Rect,
    bar: Rect,
    total: usize,
    links: Vec<(Rect, SettingsHit)>,
}

pub struct SettingsPage {
    cfg: Arc<Config>,
    entries: Vec<Entry>,
    /// Index into `entries` of the row the cursor is on; always an item.
    cursor: usize,
    scroll: usize,
    follow: bool,
    geom: Geom,
}

impl Default for SettingsPage {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsPage {
    pub fn new() -> Self {
        let mut page = Self {
            cfg: Config::cached(),
            entries: Vec::new(),
            cursor: 1,
            scroll: 0,
            follow: true,
            geom: Geom::default(),
        };
        page.reload();
        page
    }

    pub fn reload(&mut self) {
        self.cfg = Config::cached();
        let cfg = &self.cfg;
        let mut entries = vec![
            Entry::Section("playback"),
            Entry::Item(Setting::ReplayGain),
            Entry::Item(Setting::PreAmp),
            Entry::Item(Setting::FadeOnPause),
            Entry::Section("terminal"),
            Entry::Item(Setting::Theme),
            Entry::Item(Setting::CoverGraphics),
            Entry::Item(Setting::ArtSize),
            Entry::Item(Setting::ShowFps),
            Entry::Item(Setting::FrameRate),
            Entry::Item(Setting::Palette),
            Entry::Section("server"),
        ];
        if cfg.remote.enabled && !cfg.remote.url.is_empty() {
            entries.push(Entry::Info(
                "signed in",
                format!("{} as {}", cfg.remote.url, cfg.remote.username),
            ));
        } else {
            entries.push(Entry::Info("signed in", "no server".into()));
        }
        entries.push(Entry::Item(Setting::AutoSync));
        if cfg.remote.enabled {
            entries.push(Entry::Item(Setting::Offline));
            entries.push(Entry::Action(Task::Sync));
        }
        entries.push(Entry::Section("devices"));
        entries.push(Entry::Item(Setting::Discoverable));
        entries.push(Entry::Item(Setting::NearbyControl));
        entries.push(Entry::Section("library"));
        let folders = &cfg.library.folders;
        if folders.is_empty() {
            entries.push(Entry::Info(
                "folders",
                "none; add one in the app or config".into(),
            ));
        }
        if !folders.is_empty() {
            entries.push(Entry::Action(Task::Scan));
        }
        for (i, f) in folders.iter().enumerate() {
            entries.push(Entry::Info(
                if i == 0 { "folders" } else { "" },
                f.display().to_string(),
            ));
        }
        self.entries = entries;
        self.cursor = self.cursor.min(self.entries.len() - 1);
        if !selectable(&self.entries[self.cursor]) {
            self.cursor = self.step_item(self.cursor, true).unwrap_or(1);
        }
    }

    /// The next item row after (or before) `from`.
    fn step_item(&self, from: usize, forward: bool) -> Option<usize> {
        let item = |i: &usize| selectable(&self.entries[*i]);
        if forward {
            (from + 1..self.entries.len()).find(item)
        } else {
            (0..from.min(self.entries.len())).rfind(item)
        }
    }

    fn rows(&self) -> usize {
        (self.geom.list.height as usize).max(1)
    }

    fn keep_cursor_visible(&mut self) {
        let rows = self.rows();
        // The section heading above the first row of a group stays with it.
        let top = if self.cursor > 0 && matches!(self.entries[self.cursor - 1], Entry::Section(_)) {
            self.cursor - 1
        } else {
            self.cursor
        };
        if top < self.scroll {
            self.scroll = top;
        } else if self.cursor >= self.scroll + rows {
            self.scroll = self.cursor + 1 - rows;
        }
    }

    fn select(&mut self, idx: usize) {
        if self.entries.get(idx).is_some_and(selectable) {
            self.cursor = idx;
            self.follow = true;
            self.keep_cursor_visible();
        }
    }

    pub fn scroll_by(&mut self, rows: isize) {
        self.follow = false;
        let max = self.entries.len().saturating_sub(self.rows());
        self.scroll = self.scroll.saturating_add_signed(rows).min(max);
    }

    /// Changes the setting on row `idx`, or starts the task on it.
    fn change(&mut self, idx: usize, forward: bool) -> Option<Outcome> {
        match self.entries.get(idx)? {
            Entry::Item(setting) => {
                let setting = *setting;
                let wrote = setting.step(forward);
                self.reload();
                wrote.then_some(Outcome::Changed(setting))
            }
            Entry::Action(task) => forward.then_some(Outcome::Run(*task)),
            _ => None,
        }
    }

    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Option<Option<Outcome>> {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(i) = self.step_item(self.cursor, false) {
                    self.select(i);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(i) = self.step_item(self.cursor, true) {
                    self.select(i);
                }
            }
            KeyCode::Home | KeyCode::Char('g') => {
                if let Some(i) = self.step_item(0, true) {
                    self.select(i);
                }
            }
            KeyCode::End | KeyCode::Char('G') => {
                if let Some(i) = self.step_item(self.entries.len(), false) {
                    self.select(i);
                }
            }
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('-') => {
                return Some(self.change(self.cursor, false));
            }
            KeyCode::Right
            | KeyCode::Char('l')
            | KeyCode::Char('+')
            | KeyCode::Char('=')
            | KeyCode::Enter
            | KeyCode::Char(' ') => return Some(self.change(self.cursor, true)),
            _ => return None,
        }
        Some(None)
    }

    pub fn hit(&self, col: u16, row: u16) -> Option<SettingsHit> {
        let g = &self.geom;
        if let Some((_, hit)) = g.links.iter().find(|(r, _)| inside(*r, col, row)) {
            return Some(*hit);
        }
        if inside(g.list, col, row) {
            let idx = self.scroll + (row - g.list.y) as usize;
            if self.entries.get(idx).is_some_and(selectable) {
                return Some(SettingsHit::Row(idx));
            }
        }
        None
    }

    pub fn click(&mut self, hit: SettingsHit) -> Option<Outcome> {
        match hit {
            SettingsHit::Row(i) => {
                self.select(i);
                None
            }
            SettingsHit::Dec(i) => {
                self.select(i);
                self.change(i, false)
            }
            SettingsHit::Inc(i) | SettingsHit::Value(i) => {
                self.select(i);
                self.change(i, true)
            }
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
        let max = self.geom.total.saturating_sub(self.rows());
        let frac = row.saturating_sub(b.y) as f64 / b.height.saturating_sub(1).max(1) as f64;
        self.scroll = ((frac.clamp(0.0, 1.0) * max as f64).round() as usize).min(max);
    }

    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        hover: Option<SettingsHit>,
    ) {
        self.geom = Geom::default();
        if area.width < 12 || area.height < 3 {
            return;
        }
        super::ui::heading(buf, area, theme, "settings", "");
        let list = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
        self.geom.list = list;
        if self.follow {
            self.keep_cursor_visible();
        }
        let rows = list.height as usize;
        self.scroll = self.scroll.min(self.entries.len().saturating_sub(rows));
        let bar = self.entries.len() > rows && list.width > 8;
        let end = list.x + list.width - if bar { 3 } else { 0 };
        let value_x = list.x + 2 + 22;

        for r in 0..rows {
            let idx = self.scroll + r;
            let Some(entry) = self.entries.get(idx) else {
                break;
            };
            let y = list.y + r as u16;
            match entry {
                Entry::Section(name) => {
                    buf.set_string(
                        list.x + 1,
                        y,
                        name,
                        theme.heading.add_modifier(Modifier::BOLD),
                    );
                    let from = list.x + 2 + name.chars().count() as u16;
                    for cx in from..end {
                        buf[(cx, y)].set_char('\u{2500}').set_style(theme.rule);
                    }
                }
                Entry::Info(label, text) => {
                    buf.set_stringn(list.x + 2, y, label, 20, theme.hint_desc);
                    let room = end.saturating_sub(value_x) as usize;
                    buf.set_stringn(value_x, y, text, room, theme.track_normal);
                }
                Entry::Action(task) => {
                    let selected = idx == self.cursor;
                    let row_hover = matches!(
                        hover,
                        Some(SettingsHit::Row(n) | SettingsHit::Value(n)) if n == idx
                    );
                    if selected {
                        for cx in list.x..end {
                            buf[(cx, y)].set_style(theme.library_cursor);
                        }
                        buf[(list.x, y)].set_char('\u{258c}').set_style(theme.bar);
                    } else if row_hover {
                        for cx in list.x..end {
                            buf[(cx, y)].set_style(theme.library_hover);
                        }
                    }
                    let base = if selected {
                        theme.library_cursor
                    } else {
                        theme.track_normal
                    };
                    let dim = if selected {
                        theme.library_cursor
                    } else {
                        theme.hint_desc
                    };
                    buf.set_stringn(list.x + 2, y, task.label(), 20, base);
                    let run = if hover == Some(SettingsHit::Value(idx)) {
                        theme.track_playing.add_modifier(Modifier::UNDERLINED)
                    } else {
                        theme.track_playing
                    };
                    buf.set_string(value_x + 2, y, "run", run);
                    self.geom
                        .links
                        .push((Rect::new(value_x + 2, y, 3, 1), SettingsHit::Value(idx)));
                    let note_x = value_x + 8;
                    if note_x + 8 < end {
                        buf.set_stringn(note_x, y, task.note(), (end - note_x) as usize, dim);
                    }
                }
                Entry::Item(setting) => {
                    let selected = idx == self.cursor;
                    let row_hover = matches!(
                        hover,
                        Some(
                            SettingsHit::Row(n)
                                | SettingsHit::Dec(n)
                                | SettingsHit::Inc(n)
                                | SettingsHit::Value(n)
                        ) if n == idx
                    );
                    if selected {
                        for cx in list.x..end {
                            buf[(cx, y)].set_style(theme.library_cursor);
                        }
                        buf[(list.x, y)].set_char('\u{258c}').set_style(theme.bar);
                    } else if row_hover {
                        for cx in list.x..end {
                            buf[(cx, y)].set_style(theme.library_hover);
                        }
                    }
                    let base = if selected {
                        theme.library_cursor
                    } else {
                        theme.track_normal
                    };
                    let dim = if selected {
                        theme.library_cursor
                    } else {
                        theme.hint_desc
                    };
                    buf.set_stringn(list.x + 2, y, setting.label(), 20, base);

                    let value = setting.value(&self.cfg);
                    let vw = value.chars().count() as u16;
                    let arrow = |on: bool| {
                        if on {
                            dim.add_modifier(Modifier::UNDERLINED | Modifier::BOLD)
                        } else {
                            dim
                        }
                    };
                    buf.set_string(
                        value_x,
                        y,
                        "\u{2039}",
                        arrow(hover == Some(SettingsHit::Dec(idx))),
                    );
                    let value_style = if hover == Some(SettingsHit::Value(idx)) {
                        base.add_modifier(Modifier::UNDERLINED)
                    } else {
                        base.add_modifier(Modifier::BOLD)
                    };
                    buf.set_string(value_x + 2, y, &value, value_style);
                    buf.set_string(
                        value_x + 3 + vw,
                        y,
                        "\u{203a}",
                        arrow(hover == Some(SettingsHit::Inc(idx))),
                    );
                    self.geom
                        .links
                        .push((Rect::new(value_x, y, 2, 1), SettingsHit::Dec(idx)));
                    self.geom.links.push((
                        Rect::new(value_x + 2, y, vw + 1, 1),
                        SettingsHit::Value(idx),
                    ));
                    self.geom
                        .links
                        .push((Rect::new(value_x + 3 + vw, y, 2, 1), SettingsHit::Inc(idx)));

                    let note_x = value_x + vw + 8;
                    if note_x + 8 < end {
                        buf.set_stringn(note_x, y, setting.note(), (end - note_x) as usize, dim);
                    }
                }
            }
        }

        if bar {
            let x = list.x + list.width - 1;
            self.geom.bar = Rect::new(x - 2, list.y, 3, list.height);
            self.geom.total = self.entries.len();
            let h = list.height as usize;
            let total = self.entries.len();
            let thumb = (h * rows / total).max(1);
            let top = (h - thumb) * self.scroll / (total - rows).max(1);
            for row in 0..h {
                let on = row >= top && row < top + thumb;
                for cx in [x - 1, x] {
                    buf[(cx, list.y + row as u16)]
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
}

fn inside(r: Rect, col: u16, row: u16) -> bool {
    r.width > 0 && col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent};

    fn theme() -> Theme {
        Theme::new(super::super::theme::Palette::Ansi, (125, 211, 167))
    }

    fn draw(p: &mut SettingsPage, w: u16, h: u16) -> Buffer {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        p.render(area, &mut buf, &theme(), None);
        buf
    }

    #[test]
    fn every_setting_is_listed_under_its_section() {
        koan_core::config::isolate_config_for_tests();
        let mut p = SettingsPage::new();
        let buf = draw(&mut p, 100, 40);
        let shown: String = buf.content().iter().map(|c| c.symbol()).collect();
        for label in [
            "replaygain",
            "theme",
            "cover graphics",
            "visible to devices",
        ] {
            assert!(shown.contains(label), "{label}");
        }
    }

    #[test]
    fn the_cursor_rests_on_settings_never_on_headings() {
        koan_core::config::isolate_config_for_tests();
        let mut p = SettingsPage::new();
        draw(&mut p, 100, 40);
        for _ in 0..30 {
            p.handle_key(KeyEvent::from(KeyCode::Down));
            assert!(selectable(&p.entries[p.cursor]));
        }
        for _ in 0..30 {
            p.handle_key(KeyEvent::from(KeyCode::Up));
            assert!(selectable(&p.entries[p.cursor]));
        }
    }

    #[test]
    fn each_value_has_arrows_and_a_value_to_hit() {
        koan_core::config::isolate_config_for_tests();
        let mut p = SettingsPage::new();
        draw(&mut p, 100, 40);
        let kinds: Vec<_> = p.geom.links.iter().map(|(_, h)| *h).collect();
        assert!(kinds.iter().any(|h| matches!(h, SettingsHit::Dec(_))));
        assert!(kinds.iter().any(|h| matches!(h, SettingsHit::Value(_))));
        assert!(kinds.iter().any(|h| matches!(h, SettingsHit::Inc(_))));
        let (rect, hit) = p.geom.links[0];
        assert_eq!(p.hit(rect.x, rect.y), Some(hit));
    }

    #[test]
    fn older_palette_names_read_as_the_theme() {
        assert_eq!(palette_name("spectrum"), "theme");
        assert_eq!(palette_name("fire"), "fire");
    }

    #[test]
    fn it_draws_at_any_size() {
        koan_core::config::isolate_config_for_tests();
        let mut p = SettingsPage::new();
        for w in 0..60 {
            for h in 0..12 {
                draw(&mut p, w, h);
            }
        }
    }
}
