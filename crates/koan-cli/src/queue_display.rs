use std::io::{self, Write};
use std::sync::Arc;

use koan_core::player::state::{PlaybackState, QueueEntry, QueueEntryStatus, SharedPlayerState};
use owo_colors::OwoColorize;

/// UI mode for the playback screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiMode {
    /// Normal playback — queue is visible, progress bar at bottom.
    Normal,
    /// Edit mode — cursor active, can remove/reorder tracks.
    Edit,
}

/// Renders the play queue + progress bar as a full-screen ANSI display.
/// Replaces MultiProgress — we own the full terminal output.
pub struct QueueDisplay {
    state: Arc<SharedPlayerState>,
    mode: UiMode,
    /// Cursor position in edit mode (index into queue snapshot).
    cursor: usize,
    /// Last rendered queue version — skip redraws when unchanged.
    last_queue_version: u64,
    /// Last rendered position — skip redraws when unchanged.
    last_position_ms: u64,
    /// Last rendered playback state.
    last_playback_state: PlaybackState,
    /// Whether the display has been initialised (screen reserved).
    initialised: bool,
    /// Pending log messages to show above the queue.
    pending_logs: Vec<String>,
}

impl QueueDisplay {
    pub fn new(state: Arc<SharedPlayerState>) -> Self {
        Self {
            state,
            mode: UiMode::Normal,
            cursor: 0,
            last_queue_version: u64::MAX, // force initial draw
            last_position_ms: u64::MAX,
            last_playback_state: PlaybackState::Stopped,
            initialised: false,
            pending_logs: Vec::new(),
        }
    }

    pub fn mode(&self) -> UiMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: UiMode) {
        if self.mode != mode {
            self.mode = mode;
            self.force_redraw();
        }
    }

    /// Reset the display state (e.g. after returning from picker).
    /// Forces re-initialisation of screen reservation on next render.
    pub fn reset(&mut self) {
        self.initialised = false;
        self.force_redraw();
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn move_cursor_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
        self.force_redraw();
    }

    pub fn move_cursor_down(&mut self) {
        let queue = self.state.full_queue();
        if self.cursor + 1 < queue.len() {
            self.cursor += 1;
        }
        self.force_redraw();
    }

    /// Push a log line to be shown above the queue on next render.
    pub fn log(&mut self, msg: String) {
        self.pending_logs.push(msg);
    }

    /// Force a full redraw on next tick.
    fn force_redraw(&mut self) {
        self.last_queue_version = u64::MAX;
        self.last_position_ms = u64::MAX;
    }

    /// Render the display. Called every tick (~50ms).
    /// Only redraws when state actually changed.
    pub fn render(&mut self) {
        let queue_version = self.state.queue_version();
        let position_ms = self.state.position_ms();
        let playback_state = self.state.playback_state();

        // Check if anything changed.
        let queue_changed = queue_version != self.last_queue_version;
        let position_changed = position_ms != self.last_position_ms;
        let state_changed = playback_state != self.last_playback_state;
        let has_logs = !self.pending_logs.is_empty();

        if !queue_changed && !position_changed && !state_changed && !has_logs {
            return;
        }

        self.last_queue_version = queue_version;
        self.last_position_ms = position_ms;
        self.last_playback_state = playback_state;

        let (term_w, term_h) = term_size();
        let width = term_w as usize;

        let track_info = self.state.track_info();
        let queue = self.state.full_queue();

        // Layout:
        // [log messages — printed above, scroll up]
        // [now playing: 2 lines]
        // [progress bar: 1 line]
        // [queue entries: remaining height]
        // [hint bar: 1 line]

        // Print any pending log messages first — these go above our managed area
        // and scroll up naturally.
        if has_logs {
            let mut stdout = io::stdout().lock();
            if self.initialised {
                // Move to saved position, scroll the logs above it.
                let _ = write!(stdout, "\x1b[u");
            }
            for msg in self.pending_logs.drain(..) {
                let _ = writeln!(stdout, "\x1b[2K{}", truncate_ansi(&msg, width));
            }
            if self.initialised {
                // Re-save position after logs.
                let _ = write!(stdout, "\x1b[s");
            }
            let _ = stdout.flush();
        }

        // Fixed lines: now-playing (2) + progress (1) + separator (1) + hint (1) = 5
        let fixed_lines = 5;
        let queue_lines = (term_h as usize).saturating_sub(fixed_lines);
        let total_height = fixed_lines + queue_lines;

        if !self.initialised {
            let mut stdout = io::stdout().lock();
            let _ = write!(stdout, "\x1b[?25l"); // hide cursor
            for _ in 0..total_height {
                let _ = stdout.write_all(b"\n");
            }
            let _ = write!(stdout, "\x1b[{}A\x1b[s", total_height);
            let _ = stdout.flush();
            self.initialised = true;
        }

        let mut out = String::with_capacity(4096);
        out.push_str("\x1b[?25l\x1b[u"); // hide cursor + restore position

        // --- Now playing ---
        if let Some(ref info) = track_info {
            let display_name = info.path.file_stem().unwrap_or_default().to_string_lossy();
            let now_playing = format!(" {} {}", "now playing".dimmed(), display_name.bold());
            out.push_str(&format!(
                "\x1b[2K{}\r\n",
                truncate_ansi(&now_playing, width)
            ));

            let format_line = format!(
                " {} {} {} {} {}",
                info.codec.yellow().dimmed(),
                "|".dimmed(),
                format!("{}Hz", info.sample_rate).dimmed(),
                format!("{}bit", info.bit_depth).dimmed(),
                format!("{}ch", info.channels).dimmed(),
            );
            out.push_str(&format!(
                "\x1b[2K{}\r\n",
                truncate_ansi(&format_line, width)
            ));

            // --- Progress bar ---
            let status_icon = match playback_state {
                PlaybackState::Playing => ">>".cyan().to_string(),
                PlaybackState::Paused => "||".yellow().to_string(),
                PlaybackState::Stopped => "[]".dimmed().to_string(),
            };

            let time_str = format!(
                "{}/{}",
                format_time(position_ms),
                format_time(info.duration_ms)
            );

            // Build a text-based progress bar that fills available width.
            let chrome_width = 2 + 1 + time_str.len() + 1; // " icon bar time "
            let bar_width = width.saturating_sub(chrome_width + 2);
            let progress = if info.duration_ms > 0 {
                ((position_ms as f64 / info.duration_ms as f64) * bar_width as f64) as usize
            } else {
                0
            }
            .min(bar_width);

            let filled: String = "━".repeat(progress);
            let remaining: String = "─".repeat(bar_width.saturating_sub(progress));
            let progress_line = format!(
                " {} {}{} {}",
                status_icon,
                filled.cyan(),
                remaining.dimmed(),
                time_str.dimmed(),
            );
            out.push_str(&format!(
                "\x1b[2K{}\r\n",
                truncate_ansi(&progress_line, width)
            ));
        } else {
            // No track — 3 blank lines.
            for _ in 0..3 {
                out.push_str("\x1b[2K\r\n");
            }
        }

        // --- Separator ---
        let sep_label = match self.mode {
            UiMode::Normal => {
                format!(
                    " {} {}",
                    "queue".dimmed(),
                    format!("({})", queue.len()).dimmed(),
                )
            }
            UiMode::Edit => {
                format!(
                    " {} {}",
                    "queue [edit]".yellow().bold(),
                    format!("({})", queue.len()).dimmed(),
                )
            }
        };
        out.push_str(&format!("\x1b[2K{}\r\n", truncate_ansi(&sep_label, width)));

        // --- Queue entries ---
        // Clamp cursor to queue bounds.
        if !queue.is_empty() && self.cursor >= queue.len() {
            self.cursor = queue.len() - 1;
        }

        // Scroll window around cursor in edit mode.
        let start = if self.mode == UiMode::Edit && self.cursor >= queue_lines {
            self.cursor - queue_lines + 1
        } else {
            0
        };
        let end = (start + queue_lines).min(queue.len());

        for (i, entry) in queue.iter().enumerate().take(end).skip(start) {
            let line = self.render_queue_entry(i, entry, width);
            out.push_str(&format!("\x1b[2K{}\r\n", line));
        }

        // Pad remaining queue lines.
        for _ in (end - start)..queue_lines {
            out.push_str("\x1b[2K\r\n");
        }

        // --- Hint bar ---
        let hint = match self.mode {
            UiMode::Normal => format!(
                " {}  {}  {}  {}  {}  {}",
                "[space]".dimmed(),
                "[< >] skip".dimmed(),
                "[,/.] seek".dimmed(),
                "[p]track [a]lbum [r]artist".dimmed(),
                "[e]dit queue".dimmed(),
                "[q]uit".dimmed(),
            ),
            UiMode::Edit => format!(
                " {}  {}  {}  {}",
                "↑↓ navigate".dimmed(),
                "[d]elete".dimmed(),
                "[J/K] move".dimmed(),
                "[esc] done".dimmed(),
            ),
        };
        out.push_str(&format!("\x1b[2K{}", truncate_ansi(&hint, width)));

        out.push_str("\x1b[?25h"); // show cursor

        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(out.as_bytes());
        let _ = stdout.flush();
    }

    fn render_queue_entry(&self, index: usize, entry: &QueueEntry, width: usize) -> String {
        let is_cursor = self.mode == UiMode::Edit && index == self.cursor;

        let status_icon = match entry.status {
            QueueEntryStatus::Queued => "  ".to_string(),
            QueueEntryStatus::Playing => ">>".cyan().to_string(),
            QueueEntryStatus::Downloading => "..".yellow().to_string(),
            QueueEntryStatus::Failed => "!!".red().to_string(),
        };

        // Track number: disc.track or just track
        let track_num = match (entry.disc, entry.track_number) {
            (Some(d), Some(n)) if d > 1 => format!("{}.{:02}", d, n),
            (_, Some(n)) => format!("{:02}", n),
            _ => "  ".into(),
        };

        let dur = entry.duration_ms.map(format_time).unwrap_or_default();

        let artist_part = if entry.artist.is_empty() {
            String::new()
        } else {
            format!("{} {} ", entry.artist.cyan(), "—".dimmed())
        };

        let album_part = if entry.album.is_empty() {
            String::new()
        } else {
            let year = entry
                .year
                .as_deref()
                .map(|y| format!("({}) ", y).dimmed().to_string())
                .unwrap_or_default();
            let codec = entry
                .codec
                .as_deref()
                .map(|c| format!(" [{}]", c).yellow().dimmed().to_string())
                .unwrap_or_default();
            format!(
                " {} {}{}{}",
                "on".dimmed(),
                year,
                entry.album.green(),
                codec,
            )
        };

        let cursor_marker = if is_cursor { ">" } else { " " };

        let line = if is_cursor {
            format!(
                " {} {} {} {} {}{}  {}{}",
                cursor_marker.cyan().bold(),
                status_icon,
                track_num.dimmed(),
                artist_part,
                entry.title.bold(),
                album_part,
                dur.dimmed(),
                "",
            )
        } else {
            format!(
                " {} {} {} {}{}{}  {}",
                cursor_marker,
                status_icon,
                track_num.dimmed(),
                artist_part,
                entry.title,
                album_part,
                dur.dimmed(),
            )
        };

        truncate_ansi(&line, width)
    }

    /// Clear the display area and restore terminal.
    pub fn clear(&self) {
        if !self.initialised {
            return;
        }
        let (_, term_h) = term_size();
        let total_height = term_h as usize;
        let mut out = String::new();
        out.push_str("\x1b[u");
        for _ in 0..total_height {
            out.push_str("\x1b[2K\n");
        }
        out.push_str("\x1b[u\x1b[?25h");

        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(out.as_bytes());
        let _ = stdout.flush();
    }
}

fn term_size() -> (u16, u16) {
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0
            && ws.ws_col > 0
            && ws.ws_row > 0
        {
            (ws.ws_col, ws.ws_row)
        } else {
            (80, 24)
        }
    }
}

fn truncate_ansi(s: &str, max_width: usize) -> String {
    let mut out = String::new();
    let mut visible = 0;
    let mut in_escape = false;

    for c in s.chars() {
        if in_escape {
            out.push(c);
            if c.is_ascii_alphabetic() || c == 'm' {
                in_escape = false;
            }
            continue;
        }
        if c == '\x1b' {
            in_escape = true;
            out.push(c);
            continue;
        }
        if visible >= max_width {
            break;
        }
        out.push(c);
        visible += 1;
    }
    out
}

fn format_time(ms: u64) -> String {
    let secs = ms / 1000;
    let mins = secs / 60;
    let secs = secs % 60;
    if mins >= 60 {
        let hours = mins / 60;
        let mins = mins % 60;
        format!("{}:{:02}:{:02}", hours, mins, secs)
    } else {
        format!("{}:{:02}", mins, secs)
    }
}
