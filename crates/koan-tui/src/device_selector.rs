//! Output: where this device plays, as the apps' Output menu lists it. The
//! system default, then this machine's audio devices, then the UPnP
//! renderers on the network, each with how it is connected and the EQ preset
//! it plays through. Picking one moves only the sound; the queue and the
//! transport stay where they are. A renderer playing has its volume here.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use koan_core::player::state::SharedPlayerState;
use koan_core::remote::outputs::{self, LinkOutput, LinkOutputs, OutputChoice};

use super::text::{fit, width, wrap};
use super::theme::Theme;

/// What a key or a click in the picker asks of the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picked {
    Nothing,
    Close,
    Choose(OutputChoice),
    Volume(u8),
    OpenEq,
}

pub struct OutputPicker {
    pub outputs: LinkOutputs,
    pub cursor: usize,
    /// The renderers listed, by UDN: the list is read again when they change.
    seen: Vec<String>,
    /// Where each row was drawn, for the pointer.
    rows: Vec<Rect>,
}

/// How an output is connected, as a word.
fn kind_label(kind: &str) -> &str {
    match kind {
        "builtin" | "built-in" => "built in",
        "upnp" => "UPnP",
        "" => "",
        other => other,
    }
}

impl OutputPicker {
    /// Lists the outputs again, and asks the network for renderers, which
    /// answer over the next second or two (see `refresh`).
    pub fn open(state: &SharedPlayerState) -> Self {
        outputs::refresh_devices();
        koan_core::upnp::discovery::search();
        let mut picker = Self {
            outputs: outputs::local(state),
            cursor: 0,
            seen: Vec::new(),
            rows: Vec::new(),
        };
        picker.seen = picker.renderer_ids();
        picker.cursor = picker
            .choices()
            .iter()
            .position(|(c, _)| *c == picker.outputs.current)
            .unwrap_or(0);
        picker
    }

    fn renderer_ids(&self) -> Vec<String> {
        self.outputs
            .renderers
            .iter()
            .map(|r| r.id.clone())
            .collect()
    }

    /// Follows renderers coming and going while the picker is open, keeping
    /// the cursor on the output it was on.
    pub fn refresh(&mut self, state: &SharedPlayerState) {
        let found = koan_core::upnp::discovery::renderers();
        if found.iter().map(|r| &r.udn).eq(self.seen.iter()) {
            return;
        }
        let was = self.choices().get(self.cursor).map(|(c, _)| c.clone());
        self.outputs = outputs::local(state);
        self.seen = self.renderer_ids();
        let choices = self.choices();
        self.cursor = was
            .and_then(|w| choices.iter().position(|(c, _)| *c == w))
            .unwrap_or(self.cursor.min(choices.len().saturating_sub(1)));
    }

    /// Every row, in order: the default, the devices, the renderers.
    fn choices(&self) -> Vec<(OutputChoice, Option<&LinkOutput>)> {
        let mut out = vec![(OutputChoice::Default, None)];
        out.extend(
            self.outputs
                .devices
                .iter()
                .map(|d| (OutputChoice::Device { name: d.id.clone() }, Some(d))),
        );
        out.extend(
            self.outputs
                .renderers
                .iter()
                .map(|r| (OutputChoice::Renderer { udn: r.id.clone() }, Some(r))),
        );
        out
    }

    /// The renderer playing, when there is one.
    fn renderer(&self) -> Option<&LinkOutput> {
        match &self.outputs.current {
            OutputChoice::Renderer { udn } => self.outputs.renderers.iter().find(|r| r.id == *udn),
            _ => None,
        }
    }

    /// Marks `choice` as the one playing, once the app has asked for it.
    pub fn chose(&mut self, choice: OutputChoice) {
        self.outputs.current = choice;
    }

    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Picked {
        use crossterm::event::KeyCode;
        let n = self.choices().len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('D') => return Picked::Close,
            KeyCode::Up | KeyCode::Char('k') => self.cursor = (self.cursor + n - 1) % n,
            KeyCode::Down | KeyCode::Char('j') => self.cursor = (self.cursor + 1) % n,
            KeyCode::Home | KeyCode::Char('g') => self.cursor = 0,
            KeyCode::End | KeyCode::Char('G') => self.cursor = n - 1,
            KeyCode::Enter | KeyCode::Char(' ') => {
                return self
                    .choices()
                    .get(self.cursor)
                    .map_or(Picked::Nothing, |(c, _)| Picked::Choose(c.clone()));
            }
            KeyCode::Char('e') | KeyCode::Char('E') => return Picked::OpenEq,
            KeyCode::Left | KeyCode::Char('-') | KeyCode::Right | KeyCode::Char('+') => {
                let up = matches!(key.code, KeyCode::Right | KeyCode::Char('+'));
                if self.renderer().is_some()
                    && let Some(v) = self.outputs.volume
                {
                    let next = if up {
                        v.saturating_add(5).min(100)
                    } else {
                        v.saturating_sub(5)
                    };
                    self.outputs.volume = Some(next);
                    return Picked::Volume(next);
                }
            }
            _ => {}
        }
        Picked::Nothing
    }

    /// The row under the pointer.
    pub fn row_at(&self, col: u16, row: u16) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height)
    }

    /// A click on row `idx`: picks it.
    pub fn click(&mut self, idx: usize) -> Picked {
        self.cursor = idx;
        self.choices()
            .get(idx)
            .map_or(Picked::Nothing, |(c, _)| Picked::Choose(c.clone()))
    }

    pub fn scroll(&mut self, delta: isize) {
        let n = self.choices().len() as isize;
        self.cursor = (self.cursor as isize + delta).rem_euclid(n) as usize;
    }

    /// What to say under a row's name.
    fn detail(
        &self,
        choice: &OutputChoice,
        output: Option<&LinkOutput>,
        renderer_problem: Option<&str>,
    ) -> String {
        let Some(o) = output else {
            return String::new();
        };
        let current = *choice == self.outputs.current;
        if current
            && matches!(choice, OutputChoice::Renderer { .. })
            && let Some(problem) = renderer_problem
        {
            return problem.to_string();
        }
        if !current && o.busy {
            return "in use by something else; picking it takes over".into();
        }
        let how = match (o.detail.as_str(), kind_label(&o.kind)) {
            ("", kind) => kind.to_string(),
            (detail, kind) => format!("{detail} \u{b7} {kind}"),
        };
        let eq = match (&o.preset, o.unsaved) {
            (Some(p), false) => format!("eq {p}"),
            (Some(p), true) => format!("eq {p}, changed"),
            (None, true) => "eq unsaved".into(),
            (None, false) => String::new(),
        };
        [how, eq]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" \u{b7} ")
    }

    /// The lines at the foot: the renderer's volume and what it is sent.
    fn foot(&self) -> Vec<String> {
        let Some(r) = self.renderer() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Some(v) = self.outputs.volume {
            out.push(format!("volume on {}  \u{2039} {v} \u{203a}", r.name));
        }
        out.push(format!(
            "{} plays the original files, or a stream through its EQ. ReplayGain and fades don't apply.",
            r.name
        ));
        out
    }

    /// Where the popup sits in `area`.
    pub fn rect(&self, area: Rect) -> Rect {
        let w = 72u16.min(area.width.saturating_sub(4));
        let foot: usize = self
            .foot()
            .iter()
            .map(|l| wrap(l, w.saturating_sub(4) as usize).len())
            .sum();
        let foot = if foot > 0 { foot + 1 } else { 0 };
        let h = (4 + self.choices().len() + foot) as u16;
        let h = h.min(area.height.saturating_sub(2));
        Rect::new(
            area.x + area.width.saturating_sub(w) / 2,
            area.y + area.height.saturating_sub(h) / 2,
            w,
            h,
        )
    }

    /// The first row shown when `rows` fit: the cursor stays on screen.
    fn top(&self, rows: usize) -> usize {
        (self.cursor + 1).saturating_sub(rows.max(1))
    }

    pub fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme, problem: Option<&str>) {
        self.rows.clear();
        let popup = self.rect(area);
        let mut drawn = Vec::new();
        crate::shell::clear(buf, popup);
        let block = crate::shell::frame("output", theme);
        let inner = block.inner(popup);
        block.render(popup, buf);
        if inner.height < 2 || inner.width < 8 {
            return;
        }
        let w = inner.width as usize - 2;
        let foot: Vec<String> = self.foot().iter().flat_map(|l| wrap(l, w)).collect();
        let foot_h = if foot.is_empty() { 0 } else { foot.len() + 1 };
        let hint_y = inner.y + inner.height - 1;
        let list_h = (inner.height as usize).saturating_sub(foot_h + 2);
        let choices = self.choices();
        let name_w = choices
            .iter()
            .map(|(_, o)| o.map_or(14, |o| width(&o.name)))
            .max()
            .unwrap_or(14)
            .min(w / 2);
        let top = self.top(list_h);
        for (r, (i, (choice, output))) in choices
            .iter()
            .enumerate()
            .skip(top)
            .take(list_h)
            .enumerate()
        {
            let y = inner.y + r as u16;
            let line = Rect::new(inner.x, y, inner.width, 1);
            let selected = i == self.cursor;
            let current = *choice == self.outputs.current;
            let (base, dim) = if selected {
                (theme.picker_cursor, theme.picker_cursor)
            } else {
                (theme.track_normal, theme.hint_desc)
            };
            if selected {
                for cx in line.x..line.x + line.width {
                    buf[(cx, y)].set_style(base);
                }
            }
            let marker_style = if selected {
                base
            } else {
                theme.status_playing.add_modifier(Modifier::BOLD)
            };
            if current {
                buf.set_string(inner.x + 1, y, "\u{25cf}", marker_style);
            }
            let name = output.map_or("system default", |o| o.name.as_str());
            buf.set_stringn(inner.x + 3, y, fit(name, name_w), name_w, base);
            let detail = self.detail(choice, *output, problem);
            let warn = !current && output.is_some_and(|o| o.busy);
            let style = if warn && !selected {
                Style::new().fg(theme.warning)
            } else {
                dim
            };
            let dx = inner.x + 5 + name_w as u16;
            let room = (inner.x + inner.width).saturating_sub(dx + 1) as usize;
            buf.set_stringn(dx, y, fit(&detail, room), room, style);
            drawn.push(line);
        }
        self.rows = drawn;
        let from = inner.y + list_h as u16 + 1;
        for (y, line) in (from..hint_y).zip(&foot) {
            buf.set_stringn(inner.x + 1, y, line, w, theme.hint_desc);
        }
        let hint = if self.renderer().is_some() {
            " ret play here \u{b7} \u{2190}\u{2192} volume \u{b7} e eq \u{b7} esc close"
        } else {
            " ret play here \u{b7} e eq \u{b7} esc close"
        };
        buf.set_stringn(inner.x, hint_y, fit(hint, w + 2), w + 2, theme.hint_desc);
    }
}

/// The Play on chooser: which kōan the terminal shows and commands.
pub struct PlayOnOverlay<'a> {
    state: &'a super::control::PlayOnState,
    theme: &'a Theme,
}

impl<'a> PlayOnOverlay<'a> {
    pub fn new(state: &'a super::control::PlayOnState, theme: &'a Theme) -> Self {
        Self { state, theme }
    }
}

/// The chooser's rect: wider than the outputs', for what each device is doing.
pub fn play_on_rect(area: Rect, rows: usize) -> Rect {
    let w = 72u16.min(area.width.saturating_sub(4));
    // A row per device, and one saying what is found when only this one is.
    let h = (2 + rows.max(2) as u16).min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

impl Widget for PlayOnOverlay<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let popup = play_on_rect(area, self.state.choices.len());
        crate::shell::clear(buf, popup);
        let block = crate::shell::frame("play on", self.theme);
        let inner = block.inner(popup);
        block.render(popup, buf);

        for (i, choice) in self.state.choices.iter().enumerate() {
            if i >= inner.height as usize {
                break;
            }
            let selected = i == self.state.cursor;
            let base = if selected {
                self.theme.picker_cursor
            } else if choice.reachable {
                self.theme.track_normal
            } else {
                self.theme.hint_desc
            };
            let dim = if selected {
                self.theme.picker_cursor
            } else {
                self.theme.hint_desc
            };
            let marker = if choice.controlled { "\u{25CF} " } else { "  " };
            let marker_style = if selected {
                base
            } else if choice.controlled {
                self.theme.status_playing.add_modifier(Modifier::BOLD)
            } else {
                dim
            };
            let row = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
            if selected {
                for x in row.x..row.x + row.width {
                    buf[(x, row.y)].set_style(base);
                }
            }
            let name_w = 24usize.min(inner.width.saturating_sub(2) as usize);
            let name: String = choice.name.chars().take(name_w).collect();
            let line = Line::from(vec![
                Span::styled(marker, marker_style),
                Span::styled(format!("{name:<name_w$}"), base),
                Span::styled(format!(" {}", choice.detail), dim),
            ]);
            Paragraph::new(line).render(row, buf);
        }
        if self.state.choices.len() == 1 && inner.height > 1 {
            let row = Rect::new(inner.x, inner.y + 1, inner.width, 1);
            Paragraph::new(Line::from(Span::styled(
                "  no other kōan devices on this network or your server",
                self.theme.hint_desc,
            )))
            .render(row, buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(id: &str, kind: &str) -> LinkOutput {
        LinkOutput {
            id: id.into(),
            name: id.into(),
            kind: kind.into(),
            ..Default::default()
        }
    }

    fn picker(devices: &[&str], renderers: &[&str], current: OutputChoice) -> OutputPicker {
        OutputPicker {
            outputs: LinkOutputs {
                devices: devices.iter().map(|d| output(d, "usb")).collect(),
                renderers: renderers.iter().map(|r| output(r, "upnp")).collect(),
                current,
                volume: Some(30),
                ..Default::default()
            },
            cursor: 0,
            seen: Vec::new(),
            rows: Vec::new(),
        }
    }

    fn draw(p: &mut OutputPicker, w: u16, h: u16) -> String {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        p.render(area, &mut buf, &Theme::default(), None);
        buf.content().iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn the_default_comes_first_then_devices_then_renderers() {
        let p = picker(&["Topping E30"], &["Arcam"], OutputChoice::Default);
        let names: Vec<_> = p.choices().iter().map(|(c, _)| c.clone()).collect();
        assert_eq!(names[0], OutputChoice::Default);
        assert!(matches!(&names[1], OutputChoice::Device { name } if name == "Topping E30"));
        assert!(matches!(&names[2], OutputChoice::Renderer { udn } if udn == "Arcam"));
    }

    #[test]
    fn enter_picks_the_row_and_a_busy_one_says_it_takes_over() {
        let mut p = picker(&["Topping E30"], &["Arcam"], OutputChoice::Default);
        p.outputs.renderers[0].busy = true;
        p.handle_key(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Down,
        ));
        assert_eq!(
            p.handle_key(crossterm::event::KeyEvent::from(
                crossterm::event::KeyCode::Enter
            )),
            Picked::Choose(OutputChoice::Device {
                name: "Topping E30".into()
            })
        );
        let shown = draw(&mut p, 80, 20);
        assert!(shown.contains("takes over"));
    }

    #[test]
    fn a_renderer_playing_has_its_volume_on_the_arrows() {
        let current = OutputChoice::Renderer {
            udn: "Arcam".into(),
        };
        let mut p = picker(&[], &["Arcam"], current);
        let right = crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Right);
        assert_eq!(p.handle_key(right), Picked::Volume(35));
        let shown = draw(&mut p, 80, 20);
        assert!(shown.contains("volume on Arcam"));
        let mut local = picker(&["Topping E30"], &[], OutputChoice::Default);
        assert_eq!(local.handle_key(right), Picked::Nothing);
    }

    #[test]
    fn a_long_list_keeps_the_cursor_on_screen_and_names_cut_cleanly() {
        let names: Vec<String> = (0..30).map(|i| format!("Kopfhörer – Gerät {i}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut p = picker(&names, &[], OutputChoice::Default);
        p.cursor = 25;
        let shown = draw(&mut p, 60, 16);
        assert!(shown.contains("Gerät 24"), "{shown}");
        for (w, h) in [(0, 0), (10, 5), (16, 10), (30, 8), (200, 60)] {
            draw(&mut p, w, h);
        }
    }

    #[test]
    fn a_click_picks_the_row_under_the_pointer() {
        let mut p = picker(
            &["Topping E30", "MacBook Speakers"],
            &[],
            OutputChoice::Default,
        );
        draw(&mut p, 80, 20);
        let r = p.rows[2];
        let idx = p.row_at(r.x + 4, r.y).unwrap();
        assert_eq!(
            p.click(idx),
            Picked::Choose(OutputChoice::Device {
                name: "MacBook Speakers".into()
            })
        );
    }
}
