use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crossbeam_channel::Sender;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use koan_core::audio::viz::VizSnapshot;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{
    PlaybackState, QueueEntry, QueueEntryStatus, QueueItemId, SharedPlayerState,
    VisibleQueueSnapshot,
};

use super::accent::{self, Accent};
use super::actions_menu::ActionsMenu;
use super::album_page::{AlbumHit, AlbumPage};
use super::browse::{self, Browse, Source};
use super::browse_filter::{FilterPanel, Filters};
use super::history_page::{HistoryHit, HistoryPage};
use super::library::{self, LibraryState};
use super::lyrics::LyricsState;
use super::nav::{Navigator, Page};
use super::picker::{PickerKind, PickerPartKind, PickerState, picker_results_rect};
use super::playlist_modals::{Chooser, NamePrompt, Purpose};
use super::playlists_page::{ListHit, PlAction, PlaylistHit, PlaylistPage, PlaylistsPage};
use super::queue;
use super::search_page::{SearchHit, SearchPage};
use super::settings_page::{Outcome, Setting, SettingsHit, SettingsPage, Task};
use super::theme::{Palette, Theme};
use super::transport::TransportBar;
use super::visualizer::VisualizerState;

/// Characters per second the transport ticker scrolls when the title overflows.
const TICKER_FPS: u8 = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    QueueEdit,
    Picker(PickerKind),
    TrackInfo(QueueItemId),
    /// A track's row from the library, shown from a list other than the queue.
    TrackDetails,
    CoverArtZoom,
    ContextMenu,
    Organize,
    DeviceSelector,
    DspSelector,
    /// Typing the name of a playlist.
    NamePrompt,
    /// Choosing the playlist to put tracks in.
    PlaylistChooser,
    /// The things that can be done with what the cursor is on.
    ActionsMenu,
    /// What narrows the record pages.
    FilterPanel,
    HelpModal,
    VizPicker,
}

/// State for the output device selector modal.
pub struct DeviceSelectorState {
    pub devices: Vec<String>,
    pub cursor: usize,
    /// Name of the currently active device (for marking).
    pub current_device: Option<String>,
    /// The UPnP renderers listed after the local devices, by UDN, in the
    /// order their labels end `devices`.
    pub renderers: Vec<String>,
}

/// How a UPnP renderer is labelled among the output devices.
fn renderer_label(name: &str) -> String {
    format!("{name} · UPnP")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextAction {
    Play,
    Remove,
    ToggleFavourite,
    TrackInfo,
    Organize,
    CopyShareLink,
    GoToAlbum,
    GoToArtist,
    AddToPlaylist,
    Download,
    RemoveDownload,
}

/// A status bar message with the instant it was created (for auto-clear timeout).
type StatusMessage = (String, std::time::Instant);

pub struct ContextMenuState {
    /// (action, label, hotkey char)
    pub actions: Vec<(ContextAction, &'static str, char)>,
    pub cursor: usize,
}

/// What to do when the picker confirms a selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerAction {
    /// Append to end of queue (don't start playing).
    Append,
    /// Append and immediately play the first added track.
    AppendAndPlay,
    /// Put them straight after the track playing, to come next.
    PlayNext,
    /// Clear queue, add tracks, play from the top.
    ReplaceQueue,
}

pub struct DragState {
    pub from_index: usize,
    pub current_y: u16,
    /// True if we're dragging a multi-selection group.
    pub multi: bool,
    /// Offset of the clicked item within the sorted selection.
    /// E.g. if selection is [3,4,5] and you click on 4, anchor_offset = 1.
    pub anchor_offset: usize,
    /// The last desired group-start index during drag.
    /// Used to avoid redundant moves when the mouse hasn't crossed a row boundary.
    pub last_group_start: Option<usize>,
}

/// Which UI element the mouse cursor is currently hovering over.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub enum HoverZone {
    #[default]
    None,
    QueueItem(usize),
    LibraryItem(usize),
    Browse(browse::Hit),
    Album(AlbumHit),
    History(HistoryHit),
    Search(SearchHit),
    Settings(SettingsHit),
    Playlists(ListHit),
    Playlist(PlaylistHit),
    Nav(NavTarget),
    SeekBar,
    ScrollbarQueue,
    TransportArt,
    TransportText,
}

/// What a click in the sidebar or the tab bar goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavTarget {
    Page(Page),
    Back,
    Forward,
}

/// Tracks the current mouse hover position and zone.
#[derive(Default)]
pub struct HoverState {
    pub column: u16,
    pub row: u16,
    pub zone: HoverZone,
}

/// Queue cursor, selection, scroll, drag, and cached snapshot.
#[derive(Default)]
pub struct QueueState {
    pub cursor: usize,
    pub scroll_offset: usize,
    pub selected_ids: HashSet<QueueItemId>,
    pub anchor_id: Option<QueueItemId>,
    pub drag: Option<DragState>,
    /// Cached visible queue snapshot — refreshed once per frame.
    pub(super) vq_cache: VisibleQueueSnapshot,
    pub(super) vq_version: u64,
}

/// Cached layout rects from last render, used for mouse hit-testing.
#[derive(Default)]
pub struct LayoutRects {
    /// Where the EQ is shown, each a click that opens the chooser.
    pub eq_hits: Vec<ratatui::layout::Rect>,
    pub transport_area: ratatui::layout::Rect,
    pub queue_area: ratatui::layout::Rect,
    pub picker_area: ratatui::layout::Rect,
    pub track_info_area: ratatui::layout::Rect,
    pub library_area: ratatui::layout::Rect,
    pub now_playing_art_area: ratatui::layout::Rect,
    pub transport_text_area: ratatui::layout::Rect,
    pub seek_bar_start: u16,
    pub seek_bar_width: u16,
    pub context_menu_area: ratatui::layout::Rect,
    pub organize_area: ratatui::layout::Rect,
    /// The output or EQ chooser, while one is open.
    pub selector_area: ratatui::layout::Rect,
    /// The name prompt or the playlist chooser, while one is open.
    pub modal_area: ratatui::layout::Rect,
    /// The sidebar or tab bar, and what each part of it opens.
    pub nav_hits: Vec<(ratatui::layout::Rect, NavTarget)>,
    /// The key hints along the foot, and the key each stands for.
    pub hint_hits: Vec<(ratatui::layout::Rect, KeyEvent)>,
    /// The page the stage is showing, below the sidebar or tab bar.
    pub stage_area: ratatui::layout::Rect,
    pub browse_area: ratatui::layout::Rect,
    /// The hairline above the transport, which resizes it when dragged.
    pub transport_rule_y: Option<u16>,
}

/// Cover art caches and stable height tracking.
#[derive(Default)]
pub struct ArtState {
    /// Cached cover art for track info modal.
    pub cover_art: super::cover_art::CoverArtCache,
    /// Cached cover art for now-playing transport display.
    pub now_playing_art: super::cover_art::CoverArtCache,
    /// How covers are drawn: graphics protocol or half blocks.
    pub gfx: super::cover_art::Gfx,
}

pub struct App {
    pub mode: Mode,
    pub state: Arc<SharedPlayerState>,
    pub tx: Sender<PlayerCommand>,
    pub quit: bool,
    /// When a skip key last acted, to swallow the terminal's key repeat.
    last_skip: Option<std::time::Instant>,

    /// Queue cursor, selection, scroll, and cached snapshot.
    pub queue: QueueState,

    pub picker: Option<PickerState>,

    // Spinner tick for download animation.
    pub spinner_tick: usize,

    // Double-click detection.
    pub last_click_time: Option<std::time::Instant>,
    pub last_click_idx: Option<usize>,

    // Log messages from background threads.
    pub log_buffer: Arc<Mutex<Vec<String>>>,

    // Track whether we've ever been in Playing state.
    pub has_played: bool,

    pub theme: Theme,

    /// Cached layout rects from last render for mouse hit-testing.
    pub layout: LayoutRects,

    // Picker result — set when picker confirms, consumed by main loop.
    // Tagged with the picker kind and action so album IDs can be expanded
    // and the right enqueue behaviour is applied.
    pub picker_result: Option<(PickerKind, Vec<i64>, PickerAction)>,

    pub artist_drill_down: Option<i64>,

    // Loading overlay message (e.g. "loading album...").
    pub loading_message: Option<String>,

    // Auto-scroll: track by path so index shifts from finished_paths don't trigger.
    pub last_playing_path: Option<PathBuf>,

    pub library: Option<LibraryState>,

    pub nav: Navigator,
    pub albums_page: Browse,
    pub favourites_page: Browse,
    pub recent_page: Browse,
    pub artist_page: Browse,
    pub album_page: Option<AlbumPage>,
    task_running: bool,
    task_tx: crossbeam_channel::Sender<super::tasks::Report>,
    task_rx: crossbeam_channel::Receiver<super::tasks::Report>,
    pub track_details: Option<Box<koan_core::db::queries::TrackRow>>,
    /// The EQ that was on before it was bypassed, for the key that puts it back.
    eq_last: Option<String>,
    pub history_page: HistoryPage,
    pub search_page: SearchPage,
    pub playlists_page: PlaylistsPage,
    pub playlist_page: Option<PlaylistPage>,
    pub name_prompt: Option<NamePrompt>,
    pub chooser: Option<Chooser>,
    pub actions_menu: Option<ActionsMenu>,
    pub filter_panel: Option<FilterPanel>,
    /// The filters on the record pages, shared by them as the apps' are.
    pub browse_filters: Filters,
    pub settings_page: SettingsPage,
    /// A drag that began on a page's scrollbar.
    browse_drag: bool,
    pub thumbs: browse::Thumbs,
    /// The playing record's accent, easing as the record changes.
    pub accent: Accent,
    pub palette: Palette,
    /// The path whose cover the accent was taken from.
    accent_path: Option<PathBuf>,
    theme_accent: (u8, u8, u8),

    pub art: ArtState,

    /// Context menu state (when in ContextMenu mode).
    pub context_menu: Option<ContextMenuState>,

    /// Organize modal state (when in Organize mode).
    pub organize: Option<super::organize::OrganizeModalState>,

    /// Device selector modal state (when in DeviceSelector mode).
    pub device_selector: Option<DeviceSelectorState>,

    /// EQ chooser state (when in DspSelector mode).
    pub dsp_selector: Option<super::dsp_selector::DspSelectorState>,

    /// Visualizer picker modal state (when in VizPicker mode).
    pub viz_picker: Option<super::viz_picker::VizPickerState>,

    /// Stack of previous modes — push before entering a modal, pop when leaving.
    pub mode_stack: Vec<Mode>,

    /// Last known mouse row — for determining drop insertion point.
    pub last_mouse_row: Option<u16>,

    /// When Some(offset), the user is click-dragging the scrollbar thumb.
    /// The offset is the grab position within the thumb (0 = top of thumb).
    pub scrollbar_grab_offset: Option<u16>,

    /// True while a drag operation has an open undo batch.
    pub drag_undo_active: bool,

    /// Drop/paste import progress: (processed, total). Cleared when done.
    pub drop_progress: Option<Arc<(AtomicUsize, AtomicUsize)>>,

    /// Shared visualization snapshot from the analysis thread.
    pub viz_snapshot: Arc<VizSnapshot>,

    /// Visualizer state (spectrum bars, peaks, VU levels).
    pub visualizer: VisualizerState,

    pub lyrics: LyricsState,

    /// Whether the lyrics side panel is visible (toggled with `L`).
    pub lyrics_panel: bool,

    /// Receiver for background lyrics fetch results.
    pub lyrics_rx: Option<crossbeam_channel::Receiver<Option<koan_core::lyrics::Lyrics>>>,

    /// Transport bar resize drag: (start_row, original_art_size) while dragging.
    pub transport_drag: Option<(u16, u16)>,

    /// Mouse hover state — updated on MouseEventKind::Moved.
    pub hover: HoverState,

    /// Ticker offset for scrolling long transport text.
    pub ticker_offset: usize,
    /// Counter for ticker animation speed (increments each tick).
    pub ticker_tick: u8,
    /// Tick interval divisor for the ticker's fixed 8 chars-per-second scroll.
    ticker_divisor: u8,
    /// Track path used to detect track changes for ticker reset.
    ticker_last_path: Option<PathBuf>,

    /// Ticks per second (frame rate), used for animation divisors.
    ticks_per_sec: u8,
    /// Raw frame counter (always increments).
    frame_count: u32,

    /// Set of favourite track paths, loaded from DB on startup.
    pub favourites: std::collections::HashSet<i64>,

    /// Visualizer config (enabled flag, fps).
    pub viz_config: koan_core::config::VisualizerConfig,

    /// Transient status message shown in the hint bar. Cleared after a timeout.
    pub status_message: Option<StatusMessage>,

    /// Failure reasons already reported, so one dead remote raises one message
    /// rather than one per track it took down. Reset once nothing is failed.
    noted_failures: std::collections::HashSet<String>,

    /// Pending share result from background thread.
    pending_share_status: Option<Arc<Mutex<Option<StatusMessage>>>>,

    /// Whether to show FPS overlay (from config).
    pub show_fps: bool,
    /// Fullscreen visualizer mode — hides transport, queue, hints.
    pub viz_fullscreen: bool,
    /// Timestamp of last FPS sample for computing display FPS.
    fps_sample_time: std::time::Instant,
    /// Frame counter since last FPS sample.
    fps_sample_count: u32,
    /// Last computed display FPS value.
    pub display_fps: u16,

    /// Album art width in terminal columns. Height = width/2 (square via halfblocks).
    pub art_size: u16,
}

impl App {
    pub fn new(
        state: Arc<SharedPlayerState>,
        viz_snapshot: Arc<VizSnapshot>,
        tx: Sender<PlayerCommand>,
        log_buffer: Arc<Mutex<Vec<String>>>,
        ticks_per_sec: u8,
    ) -> Self {
        let task_channel = crossbeam_channel::unbounded();
        super::playlist_ops::attach(state.clone(), tx.clone());
        let cfg = koan_core::config::Config::load().unwrap_or_default();
        let ticker_divisor = (ticks_per_sec / TICKER_FPS).max(1);
        let visualizer = VisualizerState::from_config(&cfg.visualizer);
        let viz_config = cfg.visualizer;

        Self {
            mode: Mode::Normal,
            state,
            tx,
            quit: false,
            last_skip: None,
            queue: QueueState::default(),
            picker: None,
            spinner_tick: 0,
            last_click_time: None,
            last_click_idx: None,
            log_buffer,
            has_played: false,
            theme: Theme::new(Palette::detect(&cfg.playback.tui_theme), super::theme::MINT),
            layout: LayoutRects::default(),
            loading_message: None,
            picker_result: None,
            artist_drill_down: None,
            last_playing_path: None,
            library: None,
            nav: Navigator::default(),
            albums_page: Browse::new(Source::Albums),
            favourites_page: Browse::new(Source::Favourites),
            recent_page: Browse::new(Source::Recent),
            artist_page: Browse::new(Source::Artist(0)),
            album_page: None,
            task_running: false,
            task_tx: task_channel.0,
            task_rx: task_channel.1,
            track_details: None,
            eq_last: None,
            history_page: HistoryPage::new(),
            search_page: SearchPage::new(),
            playlists_page: PlaylistsPage::new(),
            playlist_page: None,
            name_prompt: None,
            chooser: None,
            actions_menu: None,
            filter_panel: None,
            browse_filters: Filters::default(),
            settings_page: SettingsPage::new(),
            browse_drag: false,
            thumbs: browse::Thumbs::new(),
            accent: Accent::default(),
            palette: Palette::detect(&cfg.playback.tui_theme),
            accent_path: None,
            theme_accent: super::theme::MINT,
            art: ArtState::default(),
            context_menu: None,
            organize: None,
            device_selector: None,
            dsp_selector: None,
            viz_picker: None,
            mode_stack: Vec::new(),
            last_mouse_row: None,
            transport_drag: None,
            scrollbar_grab_offset: None,
            drag_undo_active: false,
            drop_progress: None,
            viz_snapshot,
            visualizer,
            lyrics: LyricsState::default(),
            lyrics_panel: false,
            lyrics_rx: None,
            hover: HoverState::default(),
            ticker_offset: 0,
            ticker_tick: 0,
            ticker_divisor,
            ticker_last_path: None,
            ticks_per_sec,
            frame_count: 0,
            favourites: std::collections::HashSet::new(),
            status_message: None,
            noted_failures: std::collections::HashSet::new(),
            pending_share_status: None,
            show_fps: cfg.playback.show_fps,
            viz_fullscreen: false,
            viz_config,
            fps_sample_time: std::time::Instant::now(),
            fps_sample_count: 0,
            display_fps: 0,
            art_size: cfg.playback.art_size.clamp(4, 80),
        }
    }

    /// Push current mode onto the stack and switch to a new mode.
    fn push_mode(&mut self, mode: Mode) {
        self.mode_stack.push(self.mode.clone());
        self.mode = mode;
    }

    /// Pop the previous mode from the stack (falls back to Normal).
    fn pop_mode(&mut self) {
        self.mode = self.mode_stack.pop().unwrap_or(Mode::Normal);
    }

    pub fn load_favourites(&mut self) {
        if let Ok(db) = koan_core::db::pool::shared().get()
            && let Ok(favs) = koan_core::db::queries::load_favourites(
                &db.conn,
                koan_core::db::queries::LOCAL_USER,
            )
        {
            self.favourites = favs;
        }
    }

    /// Toggle favourite status for a track. Returns true if now favourite.
    pub fn toggle_favourite(&mut self, track_id: i64) -> bool {
        use koan_core::db::queries;
        // One transaction with the change it queues for the server, so a sync
        // importing the server's favourites sees both or neither.
        if let Ok(db) = koan_core::db::pool::shared().get()
            && let Ok(is_fav) = queries::atomically(&db.conn, || {
                let now = queries::toggle_favourite(&db.conn, queries::LOCAL_USER, track_id)?;
                koan_core::helpers::sync_favourite_to_remote(&db, track_id, now);
                Ok::<_, koan_core::db::connection::DbError>(now)
            })
        {
            if is_fav {
                self.favourites.insert(track_id);
            } else {
                self.favourites.remove(&track_id);
            }
            return is_fav;
        }
        false
    }

    /// Favourites or unfavourites a record or an artist, queueing the change
    /// for the server with it, and has the pages that show it read it again.
    fn toggle_collection_favourite(&mut self, kind: koan_core::helpers::FavouriteKind, id: i64) {
        use koan_core::db::queries;
        use koan_core::helpers::FavouriteKind;
        let Ok(db) = koan_core::db::pool::shared().get() else {
            return;
        };
        let now = queries::atomically(&db.conn, || {
            let now = match kind {
                FavouriteKind::Album => {
                    queries::toggle_favourite_album(&db.conn, queries::LOCAL_USER, id)?
                }
                _ => queries::toggle_favourite_artist(&db.conn, queries::LOCAL_USER, id)?,
            };
            koan_core::helpers::sync_collection_favourite_to_remote(&db, kind, id, now);
            Ok::<_, koan_core::db::connection::DbError>(now)
        });
        drop(db);
        let Ok(now) = now else {
            return;
        };
        self.status_message = Some((
            match (kind, now) {
                (FavouriteKind::Album, true) => "record favourited",
                (FavouriteKind::Album, false) => "record unfavourited",
                (_, true) => "artist favourited",
                (_, false) => "artist unfavourited",
            }
            .into(),
            std::time::Instant::now(),
        ));
        for page in [
            &mut self.albums_page,
            &mut self.recent_page,
            &mut self.artist_page,
        ] {
            page.reload_favourites();
        }
        // A shelf of favourites loses what was just let go.
        self.favourites_page.reload();
        if let Some(p) = self.album_page.as_mut()
            && kind == FavouriteKind::Album
            && p.album.id == id
        {
            p.favourite = now;
        }
    }

    pub fn handle_tick(&mut self) {
        // Refresh visible queue cache so all tick logic sees current state.
        self.refresh_visible_queue();

        // Clear stale double-click state after 1 second so a slow second click
        // is never misinterpreted as a double-click.
        if let Some(t) = self.last_click_time
            && t.elapsed().as_secs() >= 1
        {
            self.last_click_time = None;
            self.last_click_idx = None;
        }

        self.frame_count = self.frame_count.wrapping_add(1);

        // FPS counter: sample every second.
        self.fps_sample_count += 1;
        let elapsed = self.fps_sample_time.elapsed();
        if elapsed.as_secs() >= 1 {
            self.display_fps =
                (self.fps_sample_count as f64 / elapsed.as_secs_f64()).round() as u16;
            self.fps_sample_count = 0;
            self.fps_sample_time = std::time::Instant::now();
        }

        // Rate-limit spinner to ~10 Hz regardless of frame rate.
        if self
            .frame_count
            .is_multiple_of((self.ticks_per_sec as u32 / 10).max(1))
        {
            self.spinner_tick = self.spinner_tick.wrapping_add(1);
        }

        // Ticker: advance about `TICKER_FPS` characters a second, restarting
        // when the playing track changes.
        {
            let current_playing = self
                .queue
                .vq_cache
                .entries
                .iter()
                .find(|e| e.status == QueueEntryStatus::Playing)
                .map(|e| e.path.clone());
            if current_playing != self.ticker_last_path {
                self.ticker_last_path = current_playing;
                self.ticker_offset = 0;
                self.ticker_tick = 0;
            }
        }
        self.ticker_tick = self.ticker_tick.wrapping_add(1);
        if self.ticker_tick.is_multiple_of(self.ticker_divisor) {
            self.ticker_offset = self.ticker_offset.wrapping_add(1);
        }

        // Drain and discard log buffer (logs are already written to the file by BufferedLogger).
        if let Ok(mut logs) = self.log_buffer.lock() {
            logs.clear();
        }

        // Poll pending share result.
        let share_done = self
            .pending_share_status
            .as_ref()
            .and_then(|p| p.try_lock().ok().and_then(|mut g| g.take()));
        if let Some(msg) = share_done {
            self.status_message = Some(msg);
            self.pending_share_status = None;
        }

        // Clear transient status messages after 3 seconds (but not "Copied:" links —
        // those persist until the user presses Esc).
        if let Some((msg, ts)) = &self.status_message
            && !msg.starts_with("Copied:")
            && ts.elapsed().as_secs() >= 3
        {
            self.status_message = None;
        }

        if self.state.playback_state() == PlaybackState::Playing {
            self.has_played = true;
        }

        // Clear loading overlay once playback starts or pending queue populates.
        if self.loading_message.is_some()
            && (self.has_played || !self.queue.vq_cache.entries.is_empty())
        {
            self.loading_message = None;
        }

        if let Some(ref mut picker) = self.picker {
            picker.tick();
        }

        // Tick organize modal — check for pending preview/execute results.
        if let Some(ref mut org) = self.organize
            && let Some(result) = org.check_pending()
        {
            match result {
                super::organize::OrganizeCompletionKind::Preview => {}
                super::organize::OrganizeCompletionKind::Execute => {
                    // Send path updates to the player.
                    let updates = org.take_path_updates();
                    if !updates.is_empty() {
                        self.tx.send(PlayerCommand::UpdatePaths(updates)).ok();
                    }
                }
            }
        }

        // Check drop progress — clear when done.
        if let Some(ref progress) = self.drop_progress {
            let done = progress.0.load(Ordering::Relaxed);
            let total = progress.1.load(Ordering::Relaxed);
            if total > 0 && done >= total {
                self.drop_progress = None;
            }
        }

        // Update now-playing cover art cache when track changes.
        // Clear when nothing is playing so the old image gets freed.
        let now = std::time::Instant::now();
        if let Some(ref info) = self.state.track_info() {
            let art = self.art.now_playing_art.get(&info.path);
            if self.accent_path.as_deref() != Some(info.path.as_path()) {
                let rgb = art
                    .and_then(accent::from_image)
                    .unwrap_or(super::theme::MINT);
                self.accent_path = Some(info.path.clone());
                self.accent.set(rgb, now);
            }
        } else {
            self.art.now_playing_art.clear();
            if self.accent_path.take().is_some() {
                self.accent.set(super::theme::MINT, now);
            }
        }
        let rgb = self.accent.at(now);
        if rgb != self.theme_accent {
            self.theme_accent = rgb;
            self.theme = Theme::new(self.palette, rgb);
            if let super::visualizer::VisualizerPalette::Record(_) = self.visualizer.palette {
                self.visualizer.palette = super::visualizer::VisualizerPalette::Record(rgb);
            }
        }
        self.thumbs.poll();
        if let Ok(report) = self.task_rx.try_recv() {
            let line = match report {
                super::tasks::Report::Task(line) => {
                    self.task_running = false;
                    line
                }
                super::tasks::Report::Work(line) => line,
            };
            self.status_message = Some((line, std::time::Instant::now()));
            self.reload_library_pages();
        }
        if self.page() == Page::History && !self.history_page.filter_active {
            self.history_page.refresh();
        }
        self.artist_page.poll();
        self.search_page.poll();

        // Updated every frame: the analysis thread runs at its own rate, and
        // decay and smoothing run at the TUI frame rate.
        if self.viz_config.enabled {
            if self.state.playback_state() == PlaybackState::Playing {
                self.visualizer.update_from_snapshot(&self.viz_snapshot);
            } else {
                // When paused/stopped, decay bars to zero naturally.
                self.visualizer.decay_to_zero();
            }
            // Feed now-playing metadata for pleasures mode overlay.
            if let Some(entry) = self
                .queue
                .vq_cache
                .entries
                .iter()
                .find(|e| e.status == QueueEntryStatus::Playing)
            {
                self.visualizer.now_artist.clone_from(&entry.artist);
                self.visualizer.now_album.clone_from(&entry.album);
            }
        }

        // Check for background lyrics fetch results.
        if let Some(ref rx) = self.lyrics_rx
            && let Ok(result) = rx.try_recv()
        {
            self.lyrics.set_result(result);
            self.lyrics_rx = None;
        }

        // Trigger lyrics fetch on track change.
        if self.lyrics_panel || self.page() == Page::NowPlaying {
            let current_playing_path = self
                .queue
                .vq_cache
                .entries
                .iter()
                .find(|e| e.status == QueueEntryStatus::Playing)
                .map(|e| e.path.clone());

            if current_playing_path.is_some() && current_playing_path != self.lyrics.track_path {
                self.lyrics.track_path = current_playing_path;
                self.lyrics.fetching = true;
                self.lyrics.result = None;
                self.lyrics.lrc_lines.clear();

                if let Some(entry) = self
                    .queue
                    .vq_cache
                    .entries
                    .iter()
                    .find(|e| e.status == QueueEntryStatus::Playing)
                {
                    let artist = entry.artist.clone();
                    let title = entry.title.clone();
                    let album = entry.album.clone();
                    let duration_secs = entry.duration_ms.unwrap_or(0) / 1000;
                    let track_path = entry.path.clone();

                    let (tx, rx) = crossbeam_channel::bounded(1);
                    self.lyrics_rx = Some(rx);

                    let log_clone = self.log_buffer.clone();
                    std::thread::Builder::new()
                        .name("koan-lyrics".into())
                        .spawn(move || {
                            let result = (|| -> Option<koan_core::lyrics::Lyrics> {
                                let db = match koan_core::db::pool::shared().get() {
                                    Ok(db) => db,
                                    Err(e) => {
                                        if let Ok(mut logs) = log_clone.lock() {
                                            logs.push(format!("[lyrics] db open error: {e}"));
                                        }
                                        return None;
                                    }
                                };
                                let track_id = match koan_core::db::queries::track_id_by_path(
                                    &db.conn,
                                    &track_path.to_string_lossy(),
                                ) {
                                    Ok(Some(id)) => id,
                                    Ok(None) => {
                                        if let Ok(mut logs) = log_clone.lock() {
                                            logs.push(format!(
                                                "[lyrics] track not in db: {}",
                                                track_path.display()
                                            ));
                                        }
                                        return None;
                                    }
                                    Err(e) => {
                                        if let Ok(mut logs) = log_clone.lock() {
                                            logs.push(format!(
                                                "[lyrics] track lookup error: {e}"
                                            ));
                                        }
                                        return None;
                                    }
                                };
                                match koan_core::lyrics::fetch_lyrics(
                                    &db.conn,
                                    track_id,
                                    &artist,
                                    &title,
                                    &album,
                                    duration_secs,
                                ) {
                                    Ok(lyrics) => Some(lyrics),
                                    Err(e) => {
                                        if let Ok(mut logs) = log_clone.lock() {
                                            logs.push(format!(
                                                "[lyrics] fetch failed for '{artist} - {title}': {e}"
                                            ));
                                        }
                                        None
                                    }
                                }
                            })();
                            let _ = tx.send(result);
                        })
                        .ok();
                }
            }
        }

        // In normal mode, auto-scroll to playing track on actual track change.
        // Derive the playing track from the visible queue cache (atomic snapshot)
        // NOT from track_info directly — track_info changes before the visible
        // queue is rebuilt, causing a 1-frame scroll offset jump.
        if self.mode == Mode::Normal && self.layout.queue_area.height > 0 {
            let current_playing = self
                .queue
                .vq_cache
                .entries
                .iter()
                .find(|e| e.status == QueueEntryStatus::Playing)
                .map(|e| e.path.clone());
            if current_playing != self.last_playing_path {
                self.last_playing_path = current_playing;
                if let Some(idx) = self
                    .queue
                    .vq_cache
                    .entries
                    .iter()
                    .position(|e| e.status == QueueEntryStatus::Playing)
                {
                    let visible_height = self.queue_visible_height();
                    self.queue.scroll_offset = queue::scroll_for_cursor(
                        &self.visible_queue(),
                        idx,
                        self.queue.scroll_offset,
                        visible_height,
                    );
                }
            }
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        // Ctrl+C always quits. Stop is sent by the main loop after saving state.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }

        match &self.mode {
            Mode::Picker(_) => self.handle_picker_key(key),
            Mode::QueueEdit => self.handle_edit_key(key),
            Mode::TrackInfo(_) | Mode::TrackDetails => self.handle_info_key(key),
            Mode::CoverArtZoom => self.handle_zoom_key(key),
            Mode::ContextMenu => self.handle_context_menu_key(key),
            Mode::Organize => self.handle_organize_key(key),
            Mode::DeviceSelector => self.handle_device_selector_key(key),
            Mode::DspSelector => self.handle_dsp_selector_key(key),
            Mode::NamePrompt => self.handle_name_prompt_key(key),
            Mode::PlaylistChooser => self.handle_chooser_key(key),
            Mode::ActionsMenu => self.handle_actions_menu_key(key),
            Mode::FilterPanel => self.handle_filter_panel_key(key),
            Mode::HelpModal => self.handle_help_modal_key(key),
            Mode::VizPicker => self.handle_viz_picker_key(key),
            Mode::Normal => self.handle_page_key(key),
        }
    }

    pub fn page(&self) -> Page {
        self.nav.current()
    }

    /// Whether a page's own text field is taking the keys.
    fn typing(&self) -> bool {
        match self.page() {
            Page::Albums => self.albums_page.filter_active,
            Page::Favourites => self.favourites_page.filter_active,
            Page::Recent => self.recent_page.filter_active,
            Page::Artists => self.library.as_ref().is_some_and(|l| l.filter_active),
            Page::Artist(_) => self.artist_page.filter_active,
            Page::History => self.history_page.filter_active,
            Page::Search => self.search_page.typing,
            Page::Playlists => self.playlists_page.filter_active,
            Page::Queue
            | Page::NowPlaying
            | Page::Album(_)
            | Page::Playlist(_)
            | Page::Settings => false,
        }
    }

    pub fn browse(&self) -> Option<&Browse> {
        match self.page() {
            Page::Albums => Some(&self.albums_page),
            Page::Favourites => Some(&self.favourites_page),
            Page::Recent => Some(&self.recent_page),
            Page::Artist(_) => Some(&self.artist_page),
            _ => None,
        }
    }

    pub fn browse_mut(&mut self) -> Option<&mut Browse> {
        match self.page() {
            Page::Albums => Some(&mut self.albums_page),
            Page::Favourites => Some(&mut self.favourites_page),
            Page::Recent => Some(&mut self.recent_page),
            Page::Artist(_) => Some(&mut self.artist_page),
            _ => None,
        }
    }

    /// Opens a page, reading what it shows afresh.
    pub fn go_page(&mut self, page: Page) {
        self.nav.go(page);
        self.page_opened();
    }

    fn page_opened(&mut self) {
        match self.page() {
            Page::Albums => self.albums_page.reload(),
            Page::Favourites => self.favourites_page.reload(),
            Page::Recent => self.recent_page.reload(),
            Page::History => self.history_page.reload(),
            Page::Search => self.search_page.open(),
            Page::Playlists => self.playlists_page.reload(),
            Page::Playlist(id) => self.playlist_page = PlaylistPage::load(id),
            Page::Settings => self.settings_page.reload(),
            Page::Artists => {
                if self.library.is_none() {
                    self.library = Some(LibraryState::load());
                }
            }
            Page::NowPlaying => self.lyrics.track_path = None,
            Page::Album(id) => self.album_page = AlbumPage::load(id),
            Page::Artist(id) => {
                self.artist_page = Browse::new(Source::Artist(id));
                self.artist_page.reload();
            }
            Page::Queue => {}
        }
    }

    pub fn go_back(&mut self) {
        if self.nav.back() {
            self.page_opened();
        }
    }

    pub fn go_forward(&mut self) {
        if self.nav.forward() {
            self.page_opened();
        }
    }

    fn activate(&mut self, target: NavTarget) {
        match target {
            NavTarget::Page(p) => self.go_page(p),
            NavTarget::Back => self.go_back(),
            NavTarget::Forward => self.go_forward(),
        }
    }

    /// The keys that move between pages. True when `key` was one.
    fn navigate_key(&mut self, key: &KeyEvent) -> bool {
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match key.code {
            KeyCode::Char(c) if plain => {
                if let Some(page) = Page::from_key(c) {
                    self.go_page(page);
                    return true;
                }
                match c {
                    '[' => self.go_back(),
                    ']' => self.go_forward(),
                    _ => return false,
                }
                true
            }
            KeyCode::Tab => {
                self.go_page(self.nav.next_page());
                true
            }
            KeyCode::BackTab => {
                self.go_page(self.nav.prev_page());
                true
            }
            KeyCode::Left if key.modifiers.contains(KeyModifiers::ALT) => {
                self.go_back();
                true
            }
            KeyCode::Right if key.modifiers.contains(KeyModifiers::ALT) => {
                self.go_forward();
                true
            }
            _ => false,
        }
    }

    fn handle_page_key(&mut self, key: KeyEvent) {
        if !self.typing() {
            if self.navigate_key(&key) {
                return;
            }
            if key.code == KeyCode::Char('I') {
                self.toggle_graphics();
                return;
            }
        }
        match self.page() {
            Page::Queue => self.handle_normal_key(key),
            Page::Artists => self.handle_library_key(key),
            Page::NowPlaying => {
                if key.code == KeyCode::Esc {
                    self.leave_page();
                } else {
                    self.handle_transport_key(key);
                }
            }
            Page::Playlists => match self.playlists_page.handle_key(key) {
                Some(action) => self.apply_playlist(action),
                None if key.code == KeyCode::Esc => self.leave_page(),
                None => self.handle_transport_key(key),
            },
            Page::Playlist(_) => {
                let action = self.playlist_page.as_mut().and_then(|p| p.handle_key(key));
                match action {
                    Some(action) => self.apply_playlist(action),
                    None if key.code == KeyCode::Esc => self.leave_page(),
                    None => self.handle_transport_key(key),
                }
            }
            Page::Search => match self.search_page.handle_key(key) {
                Some(action) => self.apply_browse(action),
                None if key.code == KeyCode::Esc => self.leave_page(),
                None if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) => {
                    self.navigate_key(&key);
                }
                None => self.handle_transport_key(key),
            },
            Page::History => match self.history_page.handle_key(key) {
                Some(action) => self.apply_browse(action),
                None if key.code == KeyCode::Esc => self.leave_page(),
                None => self.handle_transport_key(key),
            },
            Page::Settings => match self.settings_page.handle_key(key) {
                Some(outcome) => {
                    if let Some(outcome) = outcome {
                        self.apply_outcome(outcome);
                    }
                }
                None if key.code == KeyCode::Esc => self.leave_page(),
                None => self.handle_transport_key(key),
            },
            Page::Album(_) => {
                let action = self.album_page.as_mut().and_then(|p| p.handle_key(key));
                match action {
                    Some(action) => self.apply_browse(action),
                    None if key.code == KeyCode::Esc => self.leave_page(),
                    None => self.handle_transport_key(key),
                }
            }
            Page::Albums | Page::Favourites | Page::Recent | Page::Artist(_) => {
                let Some(browse) = self.browse_mut() else {
                    return;
                };
                match browse.handle_key(key) {
                    Some(action) => self.apply_browse(action),
                    None if key.code == KeyCode::Esc => self.leave_page(),
                    None => self.handle_transport_key(key),
                }
            }
        }
    }

    /// Esc on a page that is not the queue: where you came from, or the queue.
    fn leave_page(&mut self) {
        if self.status_message.is_some() {
            self.status_message = None;
        } else if self.nav.can_back() {
            self.go_back();
        } else {
            self.go_page(Page::Queue);
        }
    }

    /// The keys that mean the same on every page: transport, modes, the
    /// visualiser, pickers and help.
    fn handle_transport_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shared = match key.code {
            KeyCode::Char(
                ' ' | '>' | '<' | 'n' | 's' | 'T' | 'R' | '.' | ',' | 'L' | 'V' | 'F' | 'M' | 'X'
                | 'S' | 'v' | 'D' | 'E' | 'B' | 'w' | 'W' | 'p' | 'q' | '?' | 'z',
            ) => !ctrl,
            KeyCode::Left | KeyCode::Right => true,
            _ => false,
        };
        if shared {
            self.handle_normal_key(key);
        }
    }

    /// Switches covers between the terminal's graphics protocol and half
    /// blocks, and remembers the choice.
    pub fn toggle_graphics(&mut self) {
        if !self.art.gfx.available() {
            self.status_message = Some((
                "this terminal draws no graphics; covers stay in half blocks".into(),
                std::time::Instant::now(),
            ));
            return;
        }
        let on = self.art.gfx.set_graphics(!self.art.gfx.graphics());
        let _ = koan_core::config::Config::persist(|cfg| cfg.playback.art_graphics = on);
        self.status_message = Some((
            if on {
                format!("covers drawn as images ({})", self.art.gfx.protocol_name())
            } else {
                "covers drawn in half blocks".into()
            },
            std::time::Instant::now(),
        ));
    }

    fn say(&mut self, text: impl Into<String>) {
        self.status_message = Some((text.into(), std::time::Instant::now()));
    }

    fn apply_playlist(&mut self, action: PlAction) {
        match action {
            PlAction::None => {}
            PlAction::Open(id) => self.go_page(Page::Playlist(id)),
            PlAction::Command(cmd) => {
                self.tx.send(cmd).ok();
            }
            PlAction::Tracks(ids, how) => {
                if !ids.is_empty() {
                    self.picker_result = Some((PickerKind::Track, ids, how));
                }
            }
            PlAction::OpenAlbum(id) => self.go_page(Page::Album(id)),
            PlAction::OpenArtist(id) => self.go_page(Page::Artist(id)),
            PlAction::Info(id) => self.open_track_details(id),
            PlAction::New => self.open_name_prompt(Purpose::New(Vec::new()), String::new()),
            PlAction::Rename(id, name) => self.open_name_prompt(Purpose::Rename(id), name),
            PlAction::Choose(ids) => self.open_chooser(ids),
            PlAction::Say(text) => self.say(text),
        }
    }

    fn open_name_prompt(&mut self, purpose: Purpose, text: String) {
        self.name_prompt = Some(NamePrompt::new(purpose, text));
        self.push_mode(Mode::NamePrompt);
    }

    fn close_name_prompt(&mut self) {
        self.name_prompt = None;
        self.pop_mode();
    }

    fn handle_name_prompt_key(&mut self, key: KeyEvent) {
        let Some(prompt) = self.name_prompt.as_mut() else {
            self.pop_mode();
            return;
        };
        match key.code {
            KeyCode::Esc => self.close_name_prompt(),
            KeyCode::Enter => self.confirm_name_prompt(),
            KeyCode::Backspace => {
                prompt.text.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                prompt.text.clear();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                prompt.text.push(c);
            }
            _ => {}
        }
    }

    /// Makes or renames the playlist the prompt was asking about.
    fn confirm_name_prompt(&mut self) {
        let Some(prompt) = self.name_prompt.take() else {
            return;
        };
        let said = match &prompt.purpose {
            Purpose::New(ids) => match super::playlist_ops::create(&prompt.text, ids) {
                Ok(_) if ids.is_empty() => format!("made '{}'", prompt.text.trim()),
                Ok(_) => format!(
                    "made '{}' with {} track{}",
                    prompt.text.trim(),
                    ids.len(),
                    if ids.len() == 1 { "" } else { "s" }
                ),
                Err(e) => {
                    // Kept open: a name that was refused is a name to fix.
                    self.name_prompt = Some(prompt);
                    self.say(e);
                    return;
                }
            },
            Purpose::Folder => match super::settings_page::add_folder(&prompt.text) {
                Ok(path) => format!("added {path}; run scan library to index it"),
                Err(e) => {
                    self.name_prompt = Some(prompt);
                    self.say(e);
                    return;
                }
            },
            Purpose::Rename(id) => match super::playlist_ops::rename(*id, &prompt.text) {
                Ok(()) => format!("renamed to '{}'", prompt.text.trim()),
                Err(e) => {
                    self.name_prompt = Some(prompt);
                    self.say(e);
                    return;
                }
            },
        };
        self.pop_mode();
        self.playlists_changed();
        self.settings_page.reload();
        self.say(said);
    }

    /// Has the pages that show playlists read them again.
    fn playlists_changed(&mut self) {
        self.playlists_page.reload();
        if let Some(p) = self.playlist_page.as_mut() {
            p.reload();
        }
    }

    pub(super) fn open_chooser(&mut self, ids: Vec<i64>) {
        if ids.is_empty() {
            self.say("nothing to add");
            return;
        }
        self.chooser = Some(Chooser::new(ids, super::playlist_ops::list()));
        self.push_mode(Mode::PlaylistChooser);
    }

    fn close_filter_panel(&mut self) {
        self.filter_panel = None;
        self.pop_mode();
    }

    /// Carries the panel's filters to the record pages.
    fn filters_changed(&mut self) {
        let Some(panel) = self.filter_panel.as_ref() else {
            return;
        };
        self.browse_filters = panel.filters.clone();
        for page in [
            &mut self.albums_page,
            &mut self.favourites_page,
            &mut self.recent_page,
        ] {
            page.set_filters(&self.browse_filters);
        }
        if let Some(browse) = self.browse_mut() {
            browse.ensure_loaded();
        }
    }

    fn handle_filter_panel_key(&mut self, key: KeyEvent) {
        let Some(panel) = self.filter_panel.as_mut() else {
            self.pop_mode();
            return;
        };
        let picking = panel.picking();
        let changed = match key.code {
            KeyCode::Esc if picking => {
                panel.back();
                false
            }
            KeyCode::Esc => {
                self.close_filter_panel();
                return;
            }
            KeyCode::Up => {
                panel.move_by(-1);
                false
            }
            KeyCode::Down => {
                panel.move_by(1);
                false
            }
            KeyCode::Enter if picking => {
                let at = panel.pick_cursor();
                panel.choose(at)
            }
            KeyCode::Backspace if picking => {
                panel.backspace();
                false
            }
            KeyCode::Char(c) if picking && !key.modifiers.contains(KeyModifiers::CONTROL) => {
                panel.type_char(c);
                false
            }
            KeyCode::Char('q') => {
                self.close_filter_panel();
                return;
            }
            KeyCode::Char('k') => {
                panel.move_by(-1);
                false
            }
            KeyCode::Char('j') => {
                panel.move_by(1);
                false
            }
            KeyCode::Left | KeyCode::Char('h') => panel.activate(panel.cursor, false),
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter | KeyCode::Char(' ') => {
                panel.activate(panel.cursor, true)
            }
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.close_filter_panel();
                return;
            }
            _ => false,
        };
        if changed {
            self.filters_changed();
        }
    }

    fn close_actions_menu(&mut self) {
        self.actions_menu = None;
        self.pop_mode();
    }

    /// Runs the menu's item at `idx` as its key would, and closes the menu.
    fn run_menu_item(&mut self, idx: usize) {
        let action = self.actions_menu.as_mut().and_then(|m| m.take(idx));
        self.close_actions_menu();
        if let Some(action) = action {
            self.apply_browse(action);
        }
    }

    fn handle_actions_menu_key(&mut self, key: KeyEvent) {
        let Some(menu) = self.actions_menu.as_mut() else {
            self.pop_mode();
            return;
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('m') => self.close_actions_menu(),
            KeyCode::Up | KeyCode::Char('k') => menu.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => menu.move_by(1),
            KeyCode::Home | KeyCode::Char('g') => menu.cursor = 0,
            KeyCode::End | KeyCode::Char('G') => {
                menu.cursor = menu.items.len().saturating_sub(1);
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let idx = menu.cursor;
                self.run_menu_item(idx);
            }
            _ => {}
        }
    }

    fn close_chooser(&mut self) {
        self.chooser = None;
        self.pop_mode();
    }

    /// Puts the chooser's tracks in the playlist at `idx`, or asks for the
    /// name of a new one.
    fn apply_chooser(&mut self, idx: usize) {
        let Some(chooser) = self.chooser.take() else {
            return;
        };
        let Some(target) = chooser.entries.get(idx) else {
            self.chooser = Some(chooser);
            return;
        };
        self.pop_mode();
        match target {
            None => self.open_name_prompt(Purpose::New(chooser.ids), String::new()),
            Some(list) => {
                let name = &list.name;
                match super::playlist_ops::add(list.id, &chooser.ids) {
                    Ok(n) => {
                        self.say(format!(
                            "added {n} track{} to '{name}'",
                            if n == 1 { "" } else { "s" }
                        ));
                        self.playlists_changed();
                    }
                    Err(e) => self.say(e),
                }
            }
        }
    }

    fn handle_chooser_key(&mut self, key: KeyEvent) {
        let Some(chooser) = self.chooser.as_mut() else {
            self.pop_mode();
            return;
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.close_chooser(),
            KeyCode::Up | KeyCode::Char('k') => chooser.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => chooser.move_by(1),
            KeyCode::Home | KeyCode::Char('g') => chooser.cursor = 0,
            KeyCode::End | KeyCode::Char('G') => {
                chooser.cursor = chooser.entries.len().saturating_sub(1);
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let idx = chooser.cursor;
                self.apply_chooser(idx);
            }
            _ => {}
        }
    }

    /// The tracks the queue's selection stands for; without one, the track
    /// under the edit cursor, or else the one playing. Now playing always
    /// means the one playing.
    fn queue_choice(&self) -> Vec<i64> {
        let entries = &self.queue.vq_cache.entries;
        let on_queue = self.page() != Page::NowPlaying;
        if on_queue && !self.queue.selected_ids.is_empty() {
            return entries
                .iter()
                .filter(|e| self.queue.selected_ids.contains(&e.id))
                .filter_map(|e| e.db_id)
                .collect();
        }
        let one = if on_queue && self.mode == Mode::QueueEdit {
            entries.get(self.queue.cursor)
        } else {
            entries
                .iter()
                .find(|e| e.status == QueueEntryStatus::Playing)
        };
        one.and_then(|e| e.db_id).into_iter().collect()
    }

    fn apply_outcome(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Changed(setting) => self.apply_setting(setting),
            Outcome::Run(task) => self.run_task(task),
            Outcome::RemoveFolder(i) => self.remove_folder(i),
        }
    }

    /// Starts a scan or a sync on a thread of its own and says how it went.
    fn run_task(&mut self, task: Task) {
        match task {
            Task::Scan => self.spawn_task("scanning the library...", super::tasks::scan),
            Task::Sync => self.spawn_task("syncing with the server...", super::tasks::sync),
            Task::AddFolder => self.open_name_prompt(Purpose::Folder, String::new()),
        }
    }

    /// Takes a library folder out of the config, then forgets its tracks in
    /// the background: a scan never prunes a folder it is no longer given.
    fn remove_folder(&mut self, i: usize) {
        if self.task_running {
            self.say("a scan or sync is already running");
            return;
        }
        match super::settings_page::remove_folder(i) {
            Ok(path) => {
                self.settings_page.reload();
                self.spawn_task("removing the folder's tracks...", move || {
                    super::tasks::forget(&path)
                });
            }
            Err(e) => self.say(e),
        }
    }

    fn spawn_task(&mut self, said: &str, work: impl FnOnce() -> String + Send + 'static) {
        if self.task_running {
            self.say("a scan or sync is already running");
            return;
        }
        self.task_running = true;
        self.say(said);
        let tx = self.task_tx.clone();
        std::thread::Builder::new()
            .name("koan-task".into())
            .spawn(move || {
                tx.send(super::tasks::Report::Task(work())).ok();
            })
            .ok();
    }

    /// Has the page on screen, and the artist tree when it is next opened,
    /// read the library again: after a scan or a sync, downloads removed, or
    /// offline turned on or off. The other pages read it as they open.
    fn reload_library_pages(&mut self) {
        let old = self.library.take();
        match self.page() {
            Page::Albums => self.albums_page.reload(),
            Page::Favourites => self.favourites_page.reload(),
            Page::Recent => self.recent_page.reload(),
            Page::Artist(_) => self.artist_page.reload(),
            Page::History => self.history_page.reload(),
            Page::Playlists | Page::Playlist(_) => self.playlists_changed(),
            Page::Artists => {
                let mut lib = LibraryState::load();
                if let Some(old) = old {
                    lib.filter = old.filter;
                    lib.filter_active = old.filter_active;
                    lib.apply_filter();
                }
                self.library = Some(lib);
            }
            _ => {}
        }
    }

    /// Puts a setting just written to the config into effect in the running
    /// TUI. The player reads the playback ones from the config itself.
    fn apply_setting(&mut self, setting: Setting) {
        let cfg = koan_core::config::Config::cached();
        match setting {
            Setting::Theme => {
                self.palette = Palette::detect(&cfg.playback.tui_theme);
                self.theme = Theme::new(self.palette, self.theme_accent);
            }
            Setting::CoverGraphics => {
                self.art.gfx.set_graphics(cfg.playback.art_graphics);
            }
            Setting::ArtSize => self.art_size = cfg.playback.art_size.clamp(4, 80),
            Setting::ShowFps => self.show_fps = cfg.playback.show_fps,
            Setting::Palette => {
                self.visualizer.palette =
                    super::visualizer::VisualizerPalette::parse(&cfg.visualizer.palette);
                if let super::visualizer::VisualizerPalette::Record(_) = self.visualizer.palette {
                    self.visualizer.palette =
                        super::visualizer::VisualizerPalette::Record(self.theme_accent);
                }
            }
            Setting::Offline => self.reload_library_pages(),
            _ => {}
        }
    }

    fn apply_browse(&mut self, action: browse::Action) {
        match action {
            browse::Action::None => {}
            browse::Action::Play(ids, how) => {
                self.picker_result = Some((PickerKind::Track, ids, how));
            }
            browse::Action::Favourite(id) => {
                self.toggle_favourite(id);
            }
            browse::Action::OpenAlbum(id) => self.go_page(Page::Album(id)),
            browse::Action::OpenArtist(id) => self.go_page(Page::Artist(id)),
            browse::Action::Shuffle(ids) => {
                self.tx.send(PlayerCommand::SetShuffle(true)).ok();
                self.picker_result = Some((PickerKind::Track, ids, PickerAction::ReplaceQueue));
            }
            browse::Action::ToggleAlbum(id) => {
                self.toggle_collection_favourite(koan_core::helpers::FavouriteKind::Album, id)
            }
            browse::Action::ToggleArtist(id) => {
                self.toggle_collection_favourite(koan_core::helpers::FavouriteKind::Artist, id)
            }
            browse::Action::Info(id) => self.open_track_details(id),
            browse::Action::Choose(ids) => self.open_chooser(ids),
            browse::Action::OpenFilters => {
                self.filter_panel = Some(FilterPanel::new(self.browse_filters.clone()));
                self.push_mode(Mode::FilterPanel);
            }
            browse::Action::Menu(title, items) => {
                self.actions_menu = Some(ActionsMenu::new(title, items));
                self.push_mode(Mode::ActionsMenu);
            }
            browse::Action::Download(ids) => {
                let wanted: Vec<i64> = koan_core::db::pool::shared()
                    .get()
                    .ok()
                    .and_then(|db| koan_core::db::queries::tracks_by_ids(&db.conn, &ids).ok())
                    .unwrap_or_default()
                    .into_iter()
                    .filter(super::tasks::needs_fetch)
                    .map(|t| t.id)
                    .collect();
                let n = wanted.len();
                let line = if n == 0 {
                    "nothing to download: already on this device".into()
                } else {
                    format!("downloading {n} track{}", if n == 1 { "" } else { "s" })
                };
                self.tx.send(PlayerCommand::CacheTracks(wanted)).ok();
                self.status_message = Some((line, std::time::Instant::now()));
            }
            browse::Action::RemoveDownload(ids) => {
                let (tx, state) = (self.task_tx.clone(), Arc::clone(&self.state));
                std::thread::Builder::new()
                    .name("koan-uncache".into())
                    .spawn(move || {
                        let cleared = koan_core::db::pool::shared()
                            .get()
                            .map(|db| koan_core::helpers::clear_downloads_for(&db, &ids))
                            .unwrap_or_default();
                        koan_core::helpers::requeue_cleared_downloads(&state);
                        tx.send(super::tasks::Report::Work(format!(
                            "removed {} downloaded file{}",
                            cleared.files,
                            if cleared.files == 1 { "" } else { "s" }
                        )))
                        .ok();
                    })
                    .ok();
            }
            browse::Action::OpenUrl(url) => {
                let opener = if cfg!(target_os = "macos") {
                    "open"
                } else {
                    "xdg-open"
                };
                std::process::Command::new(opener)
                    .arg(url)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .ok();
            }
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                if self.status_message.is_some() {
                    self.status_message = None;
                } else {
                    self.queue.selected_ids.clear();
                    self.queue.anchor_id = None;
                }
            }
            KeyCode::Char('q') => {
                self.quit = true;
            }
            KeyCode::Char(' ') => self.toggle_pause(),
            KeyCode::Char('>') | KeyCode::Char('n') => self.skip(PlayerCommand::NextTrack),
            KeyCode::Char('<') => self.skip(PlayerCommand::PrevTrack),
            KeyCode::Char('s') => {
                let on = !self.state.play_mode().shuffle;
                self.tx.send(PlayerCommand::SetShuffle(on)).ok();
                self.status_message = Some((
                    if on { "shuffle on" } else { "shuffle off" }.into(),
                    std::time::Instant::now(),
                ));
            }
            KeyCode::Char('T') => {
                use koan_core::player::state::{Sleep, SleepTimer};
                // Off, then each choice in turn, then off again.
                let next = match self.state.sleep() {
                    None => Some(SleepTimer::After { minutes: 15 }),
                    Some(Sleep::At { unix_ms }) => {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64;
                        // The choice it was set from: what is left, rounded up.
                        match unix_ms.saturating_sub(now).div_ceil(60_000) {
                            0..=15 => Some(SleepTimer::After { minutes: 30 }),
                            16..=30 => Some(SleepTimer::After { minutes: 45 }),
                            31..=45 => Some(SleepTimer::After { minutes: 60 }),
                            _ => Some(SleepTimer::EndOfTrack),
                        }
                    }
                    Some(Sleep::EndOfTrack) => Some(SleepTimer::EndOfRecord),
                    Some(Sleep::EndOfRecord) => None,
                };
                self.tx.send(PlayerCommand::SetSleepTimer(next)).ok();
                self.status_message = Some((
                    match next {
                        None => "sleep timer off".into(),
                        Some(SleepTimer::After { minutes }) => format!("sleep in {minutes} min"),
                        Some(SleepTimer::EndOfTrack) => "sleep at the end of the track".into(),
                        Some(SleepTimer::EndOfRecord) => "sleep at the end of the record".into(),
                    },
                    std::time::Instant::now(),
                ));
            }
            KeyCode::Char('R') => {
                let repeat = self.state.play_mode().repeat.cycled();
                self.tx.send(PlayerCommand::SetRepeat(repeat)).ok();
                self.status_message = Some((
                    format!("repeat {}", repeat.as_str()),
                    std::time::Instant::now(),
                ));
            }
            KeyCode::Char('.') | KeyCode::Right => {
                self.seek_to(self.state.position_ms().saturating_add(10_000));
            }
            KeyCode::Char(',') | KeyCode::Left => {
                self.seek_to(self.state.position_ms().saturating_sub(10_000));
            }
            KeyCode::Char('e') => {
                self.mode = Mode::QueueEdit;
                // Sync selection to cursor so j/k work immediately.
                if self.queue.selected_ids.len() <= 1 {
                    self.select_single(self.queue.cursor);
                }
            }
            KeyCode::Char('p') => {
                self.open_picker(PickerKind::Track);
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.mode = Mode::QueueEdit;
                self.select_all();
            }
            KeyCode::Char('a') => {
                self.open_picker(PickerKind::Album);
            }
            KeyCode::Char('r') => {
                self.open_picker(PickerKind::Artist);
            }
            KeyCode::Char('l') => {
                self.go_page(Page::Artists);
            }
            KeyCode::Char('A') | KeyCode::Char('O') => {
                let link = if key.code == KeyCode::Char('A') {
                    queue::HeaderLink::Artist
                } else {
                    queue::HeaderLink::Album
                };
                let db_id = self
                    .visible_queue()
                    .get(self.queue.cursor)
                    .and_then(|e| e.db_id);
                self.open_entry_link(db_id, link);
            }
            KeyCode::Char('L') => {
                self.lyrics_panel = !self.lyrics_panel;
                if self.lyrics_panel {
                    self.lyrics.track_path = None; // Force fetch on next tick
                }
            }
            KeyCode::Char('V') => {
                self.viz_config.enabled = !self.viz_config.enabled;
                self.status_message = Some((
                    if self.viz_config.enabled {
                        "visualiser on".into()
                    } else {
                        "visualiser off".into()
                    },
                    std::time::Instant::now(),
                ));
                let viz_enabled = self.viz_config.enabled;
                let _ = koan_core::config::Config::persist(|cfg| {
                    cfg.visualizer.enabled = viz_enabled;
                });
            }
            KeyCode::Char('F') => {
                self.viz_fullscreen = !self.viz_fullscreen;
                // Enable visualizer if it wasn't already.
                if self.viz_fullscreen {
                    self.viz_config.enabled = true;
                }
            }
            KeyCode::Char('M') => {
                let next_mode = self.visualizer.mode.next();
                self.apply_viz_mode(next_mode);
            }
            KeyCode::Char('X') => {
                self.visualizer.matrix_overlay = !self.visualizer.matrix_overlay;
                self.status_message = Some((
                    if self.visualizer.matrix_overlay {
                        "matrix overlay on".into()
                    } else {
                        "matrix overlay off".into()
                    },
                    std::time::Instant::now(),
                ));
                let overlay = self.visualizer.matrix_overlay;
                let _ = koan_core::config::Config::persist(|cfg| {
                    cfg.visualizer.matrix_overlay = overlay;
                });
            }
            KeyCode::Char('S') => {
                self.visualizer.bass_shake = !self.visualizer.bass_shake;
                self.status_message = Some((
                    if self.visualizer.bass_shake {
                        "bass shake on".into()
                    } else {
                        "bass shake off".into()
                    },
                    std::time::Instant::now(),
                ));
                let shake = self.visualizer.bass_shake;
                let _ = koan_core::config::Config::persist(|cfg| {
                    cfg.visualizer.bass_shake = shake;
                });
            }
            KeyCode::Char('v') => {
                self.viz_picker =
                    Some(super::viz_picker::VizPickerState::new(self.visualizer.mode));
                self.viz_config.enabled = true;
                self.viz_fullscreen = true;
                self.push_mode(Mode::VizPicker);
            }
            KeyCode::Up => {
                let visible = self.visible_queue();
                if !visible.is_empty() {
                    if self.queue.cursor == 0 {
                        self.queue.cursor = visible.len() - 1;
                    } else {
                        self.queue.cursor -= 1;
                    }
                    self.select_single(self.queue.cursor);
                    self.update_scroll();
                }
            }
            KeyCode::Down => {
                let visible = self.visible_queue();
                if !visible.is_empty() {
                    if self.queue.cursor + 1 >= visible.len() {
                        self.queue.cursor = 0;
                    } else {
                        self.queue.cursor += 1;
                    }
                    self.select_single(self.queue.cursor);
                    self.update_scroll();
                }
            }
            // Vim: page up/down, home/end.
            KeyCode::PageUp | KeyCode::Char('u')
                if key.code == KeyCode::PageUp || key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.page_up(false);
            }
            KeyCode::PageDown | KeyCode::Char('d')
                if key.code == KeyCode::PageDown
                    || key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.page_down(false);
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.jump_to_start(false);
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.jump_to_end(false);
            }
            KeyCode::Enter => {
                self.play_at_cursor();
            }
            KeyCode::Char('i') => {
                self.open_track_info(self.queue.cursor);
            }
            KeyCode::Char('f') => {
                // Toggle favourite for the track at cursor.
                let visible = self.visible_queue();
                if let Some(id) = visible.get(self.queue.cursor).and_then(|e| e.db_id) {
                    self.toggle_favourite(id);
                }
            }
            KeyCode::Char('z') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.tx.send(PlayerCommand::Undo).ok();
            }
            KeyCode::Char('z') if self.art.now_playing_art.cached().is_some() => {
                self.mode = Mode::CoverArtZoom;
            }
            KeyCode::Char('Z')
                if key
                    .modifiers
                    .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT) =>
            {
                self.tx.send(PlayerCommand::Redo).ok();
            }
            KeyCode::Char('D') => {
                self.open_device_selector();
            }
            KeyCode::Char('E') => self.open_dsp_selector(),
            KeyCode::Char('w') => {
                let ids = self.queue_choice();
                self.open_chooser(ids);
            }
            KeyCode::Char('W') => {
                let ids: Vec<i64> = self
                    .visible_queue()
                    .iter()
                    .filter_map(|e| e.db_id)
                    .collect();
                if ids.is_empty() {
                    self.say("the queue is empty");
                } else {
                    self.open_name_prompt(Purpose::New(ids), "queue".into());
                }
            }
            KeyCode::Char('B') => self.toggle_eq(),
            KeyCode::Char('/') => {
                self.open_queue_jump();
            }
            KeyCode::Char('?') => {
                self.push_mode(Mode::HelpModal);
            }
            _ => {}
        }
    }

    fn open_device_selector(&mut self) {
        match koan_core::audio::list_output_devices() {
            Ok(devices) => {
                // "System Default" as first entry, then real devices.
                let mut device_names: Vec<String> = Vec::with_capacity(devices.len() + 1);
                device_names.push("System Default".to_string());
                device_names.extend(devices.iter().map(|d| d.name.clone()));

                // Renderers answer over the next second or two; the list
                // picks them up as they do (`refresh_device_selector`).
                koan_core::upnp::discovery::search();
                let renderers = koan_core::upnp::discovery::renderers();
                device_names.extend(renderers.iter().map(|r| renderer_label(&r.name)));

                let cfg = koan_core::config::Config::load().unwrap_or_default();
                let current = match self.state.renderer() {
                    Some(output) => Some(renderer_label(&output.name)),
                    None => cfg.playback.output_device,
                };

                // Position cursor: None (system default) → index 0, else find the device.
                let cursor = current
                    .as_ref()
                    .and_then(|name| device_names.iter().position(|d| d == name))
                    .unwrap_or(0);

                self.device_selector = Some(DeviceSelectorState {
                    devices: device_names,
                    cursor,
                    current_device: current,
                    renderers: renderers.into_iter().map(|r| r.udn).collect(),
                });
                self.push_mode(Mode::DeviceSelector);
            }
            Err(e) => {
                log::error!("failed to list output devices: {}", e);
            }
        }
    }

    /// Follow renderers coming and going while the selector is open.
    pub fn refresh_device_selector(&mut self) {
        let Some(selector) = self.device_selector.as_mut() else {
            return;
        };
        let renderers = koan_core::upnp::discovery::renderers();
        if renderers
            .iter()
            .map(|r| &r.udn)
            .eq(selector.renderers.iter())
        {
            return;
        }
        let local = selector.devices.len() - selector.renderers.len();
        selector.devices.truncate(local);
        selector
            .devices
            .extend(renderers.iter().map(|r| renderer_label(&r.name)));
        selector.renderers = renderers.into_iter().map(|r| r.udn).collect();
        selector.cursor = selector
            .cursor
            .min(selector.devices.len().saturating_sub(1));
    }

    fn handle_device_selector_key(&mut self, key: KeyEvent) {
        let Some(ref mut selector) = self.device_selector else {
            return;
        };

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.device_selector = None;
                self.pop_mode();
            }
            KeyCode::Up | KeyCode::Char('k') if !selector.devices.is_empty() => {
                if selector.cursor == 0 {
                    selector.cursor = selector.devices.len() - 1;
                } else {
                    selector.cursor -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') if !selector.devices.is_empty() => {
                if selector.cursor + 1 >= selector.devices.len() {
                    selector.cursor = 0;
                } else {
                    selector.cursor += 1;
                }
            }
            KeyCode::Enter => {
                let idx = selector.cursor;
                let local = selector.devices.len() - selector.renderers.len();
                if idx >= local {
                    let udn = selector.renderers[idx - local].clone();
                    let label = selector.devices[idx].clone();
                    selector.current_device = Some(label);
                    let tx = self.tx.clone();
                    let choice = koan_core::upnp::choose();
                    // Opening a session is a few round trips to the renderer.
                    std::thread::spawn(move || {
                        if let Err(e) = koan_core::upnp::connect(&udn, choice, &tx) {
                            log::error!("upnp: {e}");
                        }
                    });
                } else if idx == 0 {
                    koan_core::upnp::choose();
                    // "System Default" — clear the configured device.
                    self.tx.send(PlayerCommand::ClearOutputDevice).ok();
                    if let Some(ref mut sel) = self.device_selector {
                        sel.current_device = None;
                    }
                } else {
                    let name = selector.devices[idx].clone();
                    koan_core::upnp::choose();
                    self.tx
                        .send(PlayerCommand::SetOutputDevice(name.clone()))
                        .ok();
                    if let Some(ref mut sel) = self.device_selector {
                        sel.current_device = Some(name);
                    }
                }
            }
            _ => {}
        }
    }

    pub fn open_dsp_selector(&mut self) {
        self.dsp_selector = Some(super::dsp_selector::DspSelectorState::load(
            self.eq_device(),
        ));
        self.push_mode(Mode::DspSelector);
    }

    /// Plays the output through the EQ at `idx`, or untouched for the first.
    fn apply_dsp(&mut self, idx: usize) {
        let Some(name) = self
            .dsp_selector
            .as_ref()
            .and_then(|s| s.entries.get(idx))
            .map(|e| e.name.clone())
        else {
            return;
        };
        if self.assign_eq(name.clone())
            && let Some(sel) = self.dsp_selector.as_mut()
        {
            sel.active = name;
            sel.cursor = idx;
        }
    }

    /// The output an EQ applies to: the renderer playing, else this machine's
    /// device.
    fn eq_device(&self) -> Option<String> {
        self.state
            .renderer()
            .map(|r| r.udn)
            .or_else(koan_core::audio::dsp::profiles::current_device)
    }

    /// Plays the output through `name`, or untouched for `None`, and says so.
    /// True when it took. Turning the EQ off remembers what was on, for
    /// [`Self::toggle_eq`].
    fn assign_eq(&mut self, name: Option<String>) -> bool {
        use koan_core::audio::dsp::profiles;
        let Some(device) = self.eq_device() else {
            self.status_message = Some((
                "no output device to set an eq on".into(),
                std::time::Instant::now(),
            ));
            return false;
        };
        let before = profiles::overview_for(Some(device.clone())).active;
        match profiles::assign(name.as_deref(), &device) {
            Ok(()) => {
                if name.is_none() && before.is_some() {
                    self.eq_last = before;
                }
                self.tx.send(PlayerCommand::ReloadDsp).ok();
                self.status_message = Some((
                    match name {
                        Some(n) => format!("eq: {n}"),
                        None => "eq off".into(),
                    },
                    std::time::Instant::now(),
                ));
                true
            }
            Err(e) => {
                self.status_message = Some((e, std::time::Instant::now()));
                false
            }
        }
    }

    /// Bypasses the EQ, or puts back what was bypassed. With nothing to put
    /// back, the chooser opens.
    fn toggle_eq(&mut self) {
        if koan_core::audio::dsp::profiles::overview_for(self.eq_device())
            .active
            .is_some()
        {
            self.assign_eq(None);
        } else if let Some(last) = self.eq_last.clone() {
            self.assign_eq(Some(last));
        } else {
            self.open_dsp_selector();
        }
    }

    fn handle_dsp_selector_key(&mut self, key: KeyEvent) {
        let Some(sel) = self.dsp_selector.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('E') => {
                self.dsp_selector = None;
                self.pop_mode();
            }
            KeyCode::Up | KeyCode::Char('k') => sel.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => sel.move_by(1),
            KeyCode::Home | KeyCode::Char('g') => sel.cursor = 0,
            KeyCode::End | KeyCode::Char('G') => sel.cursor = sel.entries.len().saturating_sub(1),
            KeyCode::Enter | KeyCode::Char(' ') => {
                let idx = sel.cursor;
                self.apply_dsp(idx);
            }
            _ => {}
        }
    }

    fn handle_help_modal_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => {
                self.pop_mode();
            }
            _ => {}
        }
    }

    fn handle_viz_picker_key(&mut self, key: KeyEvent) {
        let Some(ref mut picker) = self.viz_picker else {
            return;
        };

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('v') => {
                // Restore the original mode (revert preview).
                let original = picker.current_mode;
                self.viz_picker = None;
                self.viz_fullscreen = false;
                self.pop_mode();
                self.apply_viz_mode(original);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let len = super::viz_picker::ALL_MODES.len();
                picker.cursor = if picker.cursor == 0 {
                    len - 1
                } else {
                    picker.cursor - 1
                };
                // Live preview.
                let mode = picker.selected_mode();
                self.visualizer.mode = mode;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let len = super::viz_picker::ALL_MODES.len();
                picker.cursor = if picker.cursor + 1 >= len {
                    0
                } else {
                    picker.cursor + 1
                };
                let mode = picker.selected_mode();
                self.visualizer.mode = mode;
            }
            KeyCode::Enter => {
                let selected = picker.selected_mode();
                self.viz_picker = None;
                self.viz_fullscreen = false;
                self.pop_mode();
                self.apply_viz_mode(selected);
            }
            _ => {}
        }
    }

    /// Apply a visualizer mode: update state, persist to config, show status.
    fn apply_viz_mode(&mut self, mode: super::visualizer::VisualizerMode) {
        self.visualizer.mode = mode;
        self.viz_config.mode = mode.as_str().to_string();
        self.viz_config.enabled = true;
        self.status_message = Some((
            format!("visualiser: {}", mode.label()),
            std::time::Instant::now(),
        ));
        let mode_str = mode.as_str().to_string();
        let _ = koan_core::config::Config::persist(|cfg| {
            cfg.visualizer.mode = mode_str.clone();
        });
    }

    fn handle_edit_key(&mut self, key: KeyEvent) {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.queue.selected_ids.clear();
                self.queue.anchor_id = None;
            }
            KeyCode::Char('q') => {
                self.quit = true;
            }
            KeyCode::Up => {
                let visible = self.visible_queue();
                if !visible.is_empty() {
                    if self.queue.cursor == 0 {
                        self.queue.cursor = visible.len() - 1;
                    } else {
                        self.queue.cursor -= 1;
                    }
                }
                if shift {
                    self.extend_selection_to(self.queue.cursor);
                } else {
                    self.select_single(self.queue.cursor);
                }
                self.update_scroll();
            }
            KeyCode::Down => {
                let visible = self.visible_queue();
                if !visible.is_empty() {
                    if self.queue.cursor + 1 >= visible.len() {
                        self.queue.cursor = 0;
                    } else {
                        self.queue.cursor += 1;
                    }
                }
                if shift {
                    self.extend_selection_to(self.queue.cursor);
                } else {
                    self.select_single(self.queue.cursor);
                }
                self.update_scroll();
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                self.delete_selected();
            }
            KeyCode::Char('j') => {
                self.move_selected_down();
            }
            KeyCode::Char('k') => {
                self.move_selected_up();
            }
            KeyCode::Char('J') => {
                let visible_len = self.visible_queue().len();
                if self.queue.cursor + 1 < visible_len {
                    self.queue.cursor += 1;
                    self.extend_selection_to(self.queue.cursor);
                    self.update_scroll();
                }
            }
            KeyCode::Char('K')
                if self.queue.cursor > 0 => {
                    self.queue.cursor -= 1;
                    self.extend_selection_to(self.queue.cursor);
                    self.update_scroll();
                }
            KeyCode::Char('i') => {
                self.open_track_info(self.queue.cursor);
            }
            KeyCode::Char(' ')
                // Open context menu if there's a selection.
                if !self.queue.selected_ids.is_empty() => {
                    self.open_context_menu();
                }
            // Vim: page up/down, home/end.
            KeyCode::PageUp | KeyCode::Char('u')
                if key.code == KeyCode::PageUp || key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.page_up(shift);
            }
            KeyCode::PageDown => {
                self.page_down(shift);
            }
            KeyCode::Home | KeyCode::Char('g') if !shift => {
                self.jump_to_start(false);
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.jump_to_end(shift);
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.select_all();
            }
            KeyCode::Char('z') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.tx.send(PlayerCommand::Undo).ok();
            }
            KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.tx.send(PlayerCommand::Redo).ok();
            }
            KeyCode::Char('Z')
                if key
                    .modifiers
                    .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT) =>
            {
                self.tx.send(PlayerCommand::Redo).ok();
            }
            _ => {}
        }
    }

    fn handle_context_menu_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.context_menu = None;
                self.pop_mode();
            }
            KeyCode::Up => {
                if let Some(ref mut menu) = self.context_menu {
                    menu.cursor = menu.cursor.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                if let Some(ref mut menu) = self.context_menu
                    && menu.cursor + 1 < menu.actions.len()
                {
                    menu.cursor += 1;
                }
            }
            KeyCode::Enter => {
                if let Some(menu) = self.context_menu.take()
                    && let Some((action, _, _)) = menu.actions.get(menu.cursor)
                {
                    self.execute_context_action(*action);
                } else {
                    self.pop_mode();
                }
            }
            KeyCode::Char(c) => {
                if c == 'q' || c == ' ' {
                    self.context_menu = None;
                    self.pop_mode();
                } else if let Some(menu) = self.context_menu.take() {
                    // Match hotkey char against menu actions.
                    if let Some((action, _, _)) = menu.actions.iter().find(|(_, _, h)| *h == c) {
                        self.execute_context_action(*action);
                    } else {
                        // No match — restore menu.
                        self.context_menu = Some(menu);
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_organize_key(&mut self, key: KeyEvent) {
        let Some(ref mut org) = self.organize else {
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.organize = None;
                self.pop_mode();
            }
            KeyCode::Tab => {
                org.focus = match org.focus {
                    super::organize::OrganizeFocus::PatternList => {
                        super::organize::OrganizeFocus::Preview
                    }
                    super::organize::OrganizeFocus::Preview => {
                        super::organize::OrganizeFocus::RunButton
                    }
                    super::organize::OrganizeFocus::RunButton => {
                        super::organize::OrganizeFocus::PatternList
                    }
                };
            }
            KeyCode::Up => match org.focus {
                super::organize::OrganizeFocus::PatternList if org.pattern_cursor > 0 => {
                    org.pattern_cursor -= 1;
                    org.request_preview();
                }
                super::organize::OrganizeFocus::Preview => {
                    org.scroll = org.scroll.saturating_sub(1);
                }
                _ => {}
            },
            KeyCode::Down => match org.focus {
                super::organize::OrganizeFocus::PatternList
                    if org.pattern_cursor + 1 < org.patterns.len() =>
                {
                    org.pattern_cursor += 1;
                    org.request_preview();
                }
                super::organize::OrganizeFocus::Preview => {
                    org.scroll += 1;
                }
                _ => {}
            },
            KeyCode::Enter if !org.executing => {
                org.request_execute();
            }
            _ => {}
        }
    }

    /// Build common context menu actions, conditionally including share if remote is configured.
    fn build_context_actions(
        &self,
        fav_label: &'static str,
    ) -> Vec<(ContextAction, &'static str, char)> {
        let mut actions = vec![
            (ContextAction::Play, "Play", 'p'),
            (ContextAction::ToggleFavourite, fav_label, 'f'),
            (ContextAction::TrackInfo, "Track info", 'i'),
            (
                ContextAction::AddToPlaylist,
                "Add to a playlist\u{2026}",
                'w',
            ),
            (ContextAction::GoToAlbum, "Go to the record", 'a'),
            (ContextAction::GoToArtist, "Go to the artist", 'r'),
            (ContextAction::Download, "Download", 'l'),
            (ContextAction::RemoveDownload, "Remove the download", 'u'),
            (ContextAction::Remove, "Remove", 'd'),
            (ContextAction::Organize, "Organize files", 'o'),
        ];
        let cfg = koan_core::config::Config::load().unwrap_or_default();
        if cfg.remote.enabled && koan_core::helpers::remote_credential(&cfg).is_some() {
            actions.push((ContextAction::CopyShareLink, "Share link", 's'));
        }
        actions
    }

    fn open_context_menu(&mut self) {
        let visible = self.visible_queue();
        let is_fav = visible
            .get(self.queue.cursor)
            .is_some_and(|e| e.db_id.is_some_and(|id| self.favourites.contains(&id)));
        let fav_label = if is_fav { "Unfavourite" } else { "Favourite" };
        let actions = self.build_context_actions(fav_label);
        self.context_menu = Some(ContextMenuState { actions, cursor: 0 });
        self.push_mode(Mode::ContextMenu);
    }

    fn execute_context_action(&mut self, action: ContextAction) {
        self.pop_mode();
        match action {
            ContextAction::Play => {
                self.play_at_cursor();
            }
            ContextAction::Remove => {
                self.delete_selected();
            }
            ContextAction::ToggleFavourite => {
                let visible = self.visible_queue();
                if let Some(id) = visible.get(self.queue.cursor).and_then(|e| e.db_id) {
                    self.toggle_favourite(id);
                }
            }
            ContextAction::TrackInfo => {
                self.open_track_info(self.queue.cursor);
            }
            ContextAction::Organize => {
                self.open_organize_modal();
            }
            ContextAction::CopyShareLink => {
                self.create_and_copy_share_link();
            }
            ContextAction::AddToPlaylist => {
                let ids = self.queue_choice();
                self.open_chooser(ids);
            }
            ContextAction::GoToAlbum | ContextAction::GoToArtist => {
                let link = if action == ContextAction::GoToAlbum {
                    queue::HeaderLink::Album
                } else {
                    queue::HeaderLink::Artist
                };
                let db_id = self
                    .visible_queue()
                    .get(self.queue.cursor)
                    .and_then(|e| e.db_id);
                self.open_entry_link(db_id, link);
            }
            ContextAction::Download => {
                let ids = self.queue_choice();
                if ids.is_empty() {
                    self.say("that track is not in the library");
                    return;
                }
                self.apply_browse(browse::Action::Download(ids));
            }
            ContextAction::RemoveDownload => {
                let ids = self.queue_choice();
                if ids.is_empty() {
                    self.say("that track is not in the library");
                    return;
                }
                self.apply_browse(browse::Action::RemoveDownload(ids));
            }
        }
    }

    fn create_and_copy_share_link(&mut self) {
        let cfg = koan_core::config::Config::load().unwrap_or_default();
        if koan_core::helpers::subsonic_client(&cfg).is_none() {
            self.status_message = Some((
                koan_core::helpers::remote_unavailable(&cfg),
                std::time::Instant::now(),
            ));
            return;
        }

        let visible = self.visible_queue();
        let selected: Vec<_> = self.queue.selected_ids.iter().copied().collect();

        let db = match koan_core::db::pool::shared().get() {
            Ok(db) => db,
            Err(_) => {
                self.status_message = Some(("DB error".into(), std::time::Instant::now()));
                return;
            }
        };

        let track_ids: Vec<i64> = visible
            .iter()
            .filter(|e| selected.contains(&e.id))
            .filter_map(|e| {
                e.db_id.or_else(|| {
                    koan_core::db::queries::track_id_by_path(&db.conn, &e.path.to_string_lossy())
                        .ok()
                        .flatten()
                })
            })
            .collect();

        let log_buffer = Arc::clone(&self.log_buffer);

        // Off the UI thread: this is a network round trip.
        let status_msg: Arc<Mutex<Option<StatusMessage>>> = Arc::new(Mutex::new(None));
        let status_clone = Arc::clone(&status_msg);

        std::thread::spawn(move || {
            let result = koan_core::helpers::create_share(
                &db,
                koan_core::db::queries::LOCAL_USER,
                &cfg,
                &koan_core::helpers::ShareTarget::Tracks(track_ids),
                None,
            );
            let message = match result {
                Ok(outcome) => {
                    copy_to_clipboard(&outcome.url);
                    if outcome.skipped > 0 {
                        format!(
                            "Copied ({} of {} — the rest aren't on the server): {}",
                            outcome.shared,
                            outcome.shared + outcome.skipped,
                            outcome.url
                        )
                    } else {
                        format!("Copied: {}", outcome.url)
                    }
                }
                Err(e) => {
                    if let Ok(mut logs) = log_buffer.lock() {
                        logs.push(format!("Share error: {e}"));
                    }
                    format!("Share failed: {e}")
                }
            };
            if let Ok(mut msg) = status_clone.lock() {
                *msg = Some((message, std::time::Instant::now()));
            }
        });

        // We can't directly set status_message from the thread, so set a pending message.
        // The status will be set once the thread completes — store the Arc for polling.
        self.pending_share_status = Some(status_msg);
        self.status_message = Some(("Creating share...".into(), std::time::Instant::now()));
    }

    fn open_organize_modal(&mut self) {
        let config = koan_core::config::Config::load().unwrap_or_default();
        let mut patterns: Vec<(String, String)> = config.organize.patterns.into_iter().collect();
        patterns.sort_by(|a, b| a.0.cmp(&b.0));
        let default_pattern = config
            .organize
            .default
            .and_then(|name| patterns.iter().position(|(n, _)| *n == name))
            .unwrap_or(0);

        // Collect selected queue entries' paths.
        let visible = self.visible_queue();
        let selected_paths: Vec<PathBuf> = visible
            .iter()
            .filter(|e| self.queue.selected_ids.contains(&e.id))
            .map(|e| e.path.clone())
            .collect();

        // Collect the QueueItemIds for the selection (needed for path updates later).
        let selected_ids: Vec<(koan_core::player::state::QueueItemId, PathBuf)> = visible
            .iter()
            .filter(|e| self.queue.selected_ids.contains(&e.id))
            .map(|e| (e.id, e.path.clone()))
            .collect();

        let org = super::organize::OrganizeModalState::new(
            patterns,
            default_pattern,
            selected_paths,
            selected_ids,
        );
        self.organize = Some(org);
        self.push_mode(Mode::Organize);
    }

    fn handle_picker_key(&mut self, key: KeyEvent) {
        let Some(ref mut picker) = self.picker else {
            return;
        };

        // Determine if this keypress is a confirm action.
        let action = match (key.code, key.modifiers) {
            (KeyCode::Enter, m) if m.contains(KeyModifiers::CONTROL) => {
                Some(PickerAction::AppendAndPlay)
            }
            (KeyCode::Char('r'), m) if m.contains(KeyModifiers::CONTROL) => {
                Some(PickerAction::ReplaceQueue)
            }
            (KeyCode::Enter, _) => Some(PickerAction::Append),
            _ => None,
        };

        if let Some(action) = action {
            let ids = picker.confirm();
            let kind = picker.kind;
            self.picker = None;
            self.mode = Mode::Normal;

            if !ids.is_empty() {
                match kind {
                    PickerKind::Track | PickerKind::Album => {
                        self.picker_result = Some((kind, ids, action));
                    }
                    PickerKind::Artist => {
                        self.artist_drill_down = Some(ids[0]);
                    }
                    PickerKind::QueueJump => {
                        let idx = ids[0] as usize;
                        let visible = self.visible_queue();
                        if let Some(entry) = visible.get(idx) {
                            self.tx.send(PlayerCommand::Play(entry.id)).ok();
                            self.queue.cursor = idx;
                            // Scroll so the jumped-to track is near the top.
                            let visible_height = self.queue_visible_height();
                            self.queue.scroll_offset =
                                queue::scroll_cursor_to_top(&visible, idx, visible_height);
                        }
                    }
                }
            }
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.picker = None;
                self.mode = Mode::Normal;
            }
            KeyCode::Up => picker.move_up(),
            KeyCode::Down => picker.move_down(),
            KeyCode::PageUp => picker.page_up(10),
            KeyCode::PageDown => picker.page_down(10),
            KeyCode::Home => picker.jump_to_start(),
            KeyCode::End => picker.jump_to_end(),
            KeyCode::Tab => picker.toggle_select(),
            KeyCode::Backspace => picker.backspace(),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                picker.page_up(10);
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                picker.page_down(10);
            }
            KeyCode::Char(c) => picker.type_char(c),
            _ => {}
        }
    }

    fn handle_info_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('i') | KeyCode::Char('q') => {
                self.mode = Mode::Normal;
                self.track_details = None;
                self.art.cover_art.clear();
            }
            _ => {}
        }
    }

    /// Shows what the library holds of `track_id`.
    fn open_track_details(&mut self, track_id: i64) {
        let row = koan_core::db::pool::shared()
            .get()
            .ok()
            .and_then(|db| koan_core::db::queries::get_track_row(&db.conn, track_id).ok())
            .flatten();
        if let Some(row) = row {
            self.track_details = Some(Box::new(row));
            self.mode = Mode::TrackDetails;
        }
    }

    fn handle_zoom_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('z') | KeyCode::Char('q') => {
                self.mode = Mode::Normal;
            }
            _ => {}
        }
    }

    fn open_track_info(&mut self, idx: usize) {
        let Some(entry) = self.queue.vq_cache.entries.get(idx) else {
            return;
        };
        let (id, path) = (entry.id, entry.path.clone());
        self.mode = Mode::TrackInfo(id);
        // Prime the cache for this track's path.
        self.art.cover_art.get(&path);
    }

    pub fn handle_mouse(&mut self, event: MouseEvent) {
        // Transport bar resize: drag the bottom edge to resize album art / header.
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Start drag if clicking the hairline above the transport.
                let t = self.layout.transport_area;
                if t.height > 0
                    && self.layout.transport_rule_y == Some(event.row)
                    && event.column >= t.x
                    && event.column < t.x + t.width
                {
                    self.transport_drag = Some((event.row, self.art_size));
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if self.transport_drag.is_some() => {
                let (start_row, original_size) = self.transport_drag.unwrap();
                let delta = start_row as i32 - event.row as i32;
                // Each row of drag = 2 units of art_size (since art_height = art_size / 2).
                let new_size = (original_size as i32 + delta * 2).clamp(4, 80) as u16;
                self.art_size = new_size;
                return;
            }
            MouseEventKind::Up(MouseButton::Left) if self.transport_drag.is_some() => {
                let size = self.art_size;
                let _ = koan_core::config::Config::persist(|cfg| {
                    cfg.playback.art_size = size;
                });
                self.transport_drag = None;
                return;
            }
            _ => {
                if self.transport_drag.is_some()
                    && !matches!(event.kind, MouseEventKind::Drag(MouseButton::Left))
                {
                    self.transport_drag = None;
                }
            }
        }

        // Track mouse row for drop insertion indicator.
        if self.is_in_rect(event.column, event.row, self.layout.queue_area) {
            self.last_mouse_row = Some(event.row);
        } else {
            self.last_mouse_row = None;
        }

        // Cover art zoom intercepts all mouse events.
        if self.mode == Mode::CoverArtZoom {
            if let MouseEventKind::Down(MouseButton::Left) = event.kind {
                self.mode = Mode::Normal;
            }
            return;
        }

        // The name prompt and the playlist chooser: click a row, or `ok`;
        // click outside to cancel.
        if self.mode == Mode::ActionsMenu {
            let area = self.layout.modal_area;
            let inner = ratatui::layout::Rect::new(
                area.x + 1,
                area.y + 1,
                area.width.saturating_sub(2),
                area.height.saturating_sub(2),
            );
            match event.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if !self.is_in_rect(event.column, event.row, area) {
                        self.close_actions_menu();
                    } else if self.is_in_rect(event.column, event.row, inner) {
                        let top = self
                            .actions_menu
                            .as_ref()
                            .map_or(0, |m| m.top(inner.height as usize));
                        self.run_menu_item(top + (event.row - inner.y) as usize);
                    }
                }
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                    if let Some(m) = self.actions_menu.as_mut() {
                        m.move_by(if matches!(event.kind, MouseEventKind::ScrollUp) {
                            -1
                        } else {
                            1
                        });
                    }
                }
                _ => {}
            }
            return;
        }

        if self.mode == Mode::FilterPanel {
            let area = self.layout.modal_area;
            let inner = ratatui::layout::Rect::new(
                area.x + 1,
                area.y + 1,
                area.width.saturating_sub(2),
                area.height.saturating_sub(2),
            );
            if let MouseEventKind::Down(MouseButton::Left) = event.kind {
                if !self.is_in_rect(event.column, event.row, area) {
                    if !self.filter_panel.as_mut().is_some_and(|p| p.back()) {
                        self.close_filter_panel();
                    }
                } else if self.is_in_rect(event.column, event.row, inner) {
                    let row = (event.row - inner.y) as usize;
                    let changed = self.filter_panel.as_mut().is_some_and(|panel| {
                        if panel.picking() {
                            let at = panel.pick_top(inner.height as usize) + row;
                            panel.choose(at)
                        } else {
                            panel.activate(row, true)
                        }
                    });
                    if changed {
                        self.filters_changed();
                    }
                }
            }
            return;
        }

        if matches!(self.mode, Mode::NamePrompt | Mode::PlaylistChooser) {
            let area = self.layout.modal_area;
            let inner = ratatui::layout::Rect::new(
                area.x + 1,
                area.y + 1,
                area.width.saturating_sub(2),
                area.height.saturating_sub(2),
            );
            match event.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if !self.is_in_rect(event.column, event.row, area) {
                        if self.mode == Mode::NamePrompt {
                            self.close_name_prompt();
                        } else {
                            self.close_chooser();
                        }
                    } else if self.mode == Mode::NamePrompt {
                        let ok = super::playlist_modals::prompt_ok_rect(area);
                        if self.is_in_rect(event.column, event.row, ok) {
                            self.confirm_name_prompt();
                        }
                    } else if self.is_in_rect(event.column, event.row, inner) {
                        let top = self
                            .chooser
                            .as_ref()
                            .map_or(0, |c| c.top(inner.height as usize));
                        self.apply_chooser(top + (event.row - inner.y) as usize);
                    }
                }
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                    if let Some(c) = self.chooser.as_mut() {
                        c.move_by(if matches!(event.kind, MouseEventKind::ScrollUp) {
                            -1
                        } else {
                            1
                        });
                    }
                }
                _ => {}
            }
            return;
        }

        // Track info intercepts all mouse events when active.
        if matches!(self.mode, Mode::TrackInfo(_) | Mode::TrackDetails) {
            if let MouseEventKind::Down(MouseButton::Left) = event.kind
                && !self.is_in_rect(event.column, event.row, self.layout.track_info_area)
            {
                self.mode = Mode::Normal;
                self.track_details = None;
                self.art.cover_art.clear();
            }
            return;
        }

        // Organize modal intercepts all mouse events.
        if self.mode == Mode::Organize {
            if let MouseEventKind::Down(MouseButton::Left) = event.kind
                && !self.is_in_rect(event.column, event.row, self.layout.organize_area)
            {
                self.organize = None;
                self.pop_mode();
            }
            return;
        }

        // Context menu intercepts all mouse events.
        if self.mode == Mode::ContextMenu {
            if let MouseEventKind::Down(MouseButton::Left) = event.kind {
                if self.is_in_rect(event.column, event.row, self.layout.context_menu_area) {
                    // Click on an action row — compute which one.
                    let row = (event
                        .row
                        .saturating_sub(self.layout.context_menu_area.y + 1))
                        as usize;
                    if let Some(ref mut menu) = self.context_menu
                        && row < menu.actions.len()
                    {
                        menu.cursor = row;
                        let action = menu.actions[row].0;
                        self.context_menu = None;
                        self.execute_context_action(action);
                    }
                } else {
                    self.context_menu = None;
                    self.pop_mode();
                }
            }
            return;
        }

        // The output and EQ choosers: click a row to choose it, click outside
        // to close, wheel to move.
        if matches!(self.mode, Mode::DeviceSelector | Mode::DspSelector) {
            let area = self.layout.selector_area;
            let inner = ratatui::layout::Rect::new(
                area.x + 1,
                area.y + 1,
                area.width.saturating_sub(2),
                area.height.saturating_sub(2),
            );
            match event.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if !self.is_in_rect(event.column, event.row, area) {
                        self.dsp_selector = None;
                        self.device_selector = None;
                        self.pop_mode();
                    } else if self.is_in_rect(event.column, event.row, inner) {
                        let row = (event.row - inner.y) as usize;
                        if self.mode == Mode::DspSelector {
                            let top = self
                                .dsp_selector
                                .as_ref()
                                .map_or(0, |s| s.top(inner.height as usize));
                            self.apply_dsp(top + row);
                        } else if let Some(sel) = self.device_selector.as_mut()
                            && row < sel.devices.len()
                        {
                            sel.cursor = row;
                            self.handle_device_selector_key(KeyEvent::from(KeyCode::Enter));
                        }
                    }
                }
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                    let delta = if matches!(event.kind, MouseEventKind::ScrollUp) {
                        -1
                    } else {
                        1
                    };
                    if let Some(sel) = self.dsp_selector.as_mut() {
                        sel.move_by(delta);
                    } else if let Some(sel) = self.device_selector.as_mut()
                        && !sel.devices.is_empty()
                    {
                        sel.cursor = (sel.cursor as isize + delta)
                            .rem_euclid(sel.devices.len() as isize)
                            as usize;
                    }
                }
                _ => {}
            }
            return;
        }

        // Picker intercepts all mouse events when active.
        if let Mode::Picker(_) = &self.mode {
            let picker_area = self.layout.picker_area;
            let results = picker_results_rect(picker_area);
            let in_results = self.is_in_rect(event.column, event.row, results);
            let in_popup = self.is_in_rect(event.column, event.row, picker_area);

            if let MouseEventKind::Down(MouseButton::Left) = event.kind {
                if in_results {
                    if let Some(ref mut picker) = self.picker {
                        let visible_height = results.height as usize;
                        let start = if picker.cursor >= visible_height {
                            picker.cursor - visible_height + 1
                        } else {
                            0
                        };
                        let row_in_results = (event.row - results.y) as usize;
                        let item_idx = start + row_in_results;
                        if item_idx < picker.matched_count() {
                            let now = std::time::Instant::now();
                            let is_double = self.last_click_idx == Some(item_idx)
                                && self
                                    .last_click_time
                                    .is_some_and(|t| now.duration_since(t).as_millis() < 400);

                            if is_double {
                                self.last_click_idx = None;
                                self.last_click_time = None;
                                picker.cursor = item_idx;
                                let ids = picker.confirm();
                                let kind = picker.kind;
                                self.picker = None;
                                self.mode = Mode::Normal;
                                if !ids.is_empty() {
                                    match kind {
                                        PickerKind::Track | PickerKind::Album => {
                                            self.picker_result =
                                                Some((kind, ids, PickerAction::AppendAndPlay));
                                        }
                                        PickerKind::Artist => {
                                            self.artist_drill_down = Some(ids[0]);
                                        }
                                        PickerKind::QueueJump => {
                                            let idx = ids[0] as usize;
                                            let visible = self.visible_queue();
                                            if let Some(entry) = visible.get(idx) {
                                                self.tx.send(PlayerCommand::Play(entry.id)).ok();
                                                self.queue.cursor = idx;
                                                self.update_scroll();
                                            }
                                        }
                                    }
                                }
                            } else {
                                self.last_click_idx = Some(item_idx);
                                self.last_click_time = Some(now);
                                picker.cursor = item_idx;
                            }
                        }
                    }
                } else if !in_popup {
                    // Click outside picker -> close.
                    self.picker = None;
                    self.mode = Mode::Normal;
                }
            }

            // Scroll events fall through to existing scroll handler.
            if matches!(
                event.kind,
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
            ) {
                // Fall through.
            } else {
                return;
            }
        }

        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // A key hint along the foot is that key.
                if let Some(key) = self
                    .layout
                    .hint_hits
                    .iter()
                    .find(|(r, _)| {
                        event.column >= r.x
                            && event.column < r.x + r.width
                            && event.row == r.y
                    })
                    .map(|(_, k)| *k)
                {
                    self.handle_key(key);
                    return;
                }

                // Wherever the EQ is shown, a click opens the chooser.
                if self.layout.eq_hits.iter().any(|r| {
                    event.column >= r.x
                        && event.column < r.x + r.width
                        && event.row >= r.y
                        && event.row < r.y + r.height
                }) {
                    self.open_dsp_selector();
                    return;
                }

                // Sidebar and tab bar.
                if let Some(target) = self.nav_target_at(event.column, event.row) {
                    self.activate(target);
                    return;
                }

                // The record and the browsing pages.
                if self.page() != Page::Queue
                    && self.is_in_rect(event.column, event.row, self.layout.browse_area)
                {
                    let (col, row) = (event.column, event.row);
                    if self.page() == Page::Playlists {
                        let p = &mut self.playlists_page;
                        let mut action = PlAction::None;
                        if p.on_scrollbar(col, row) {
                            self.browse_drag = true;
                            p.scroll_to_bar(row);
                        } else if let Some(hit) = p.hit(col, row) {
                            action = p.click(hit);
                        }
                        self.apply_playlist(action);
                        return;
                    }
                    if let Page::Playlist(_) = self.page() {
                        let mut action = PlAction::None;
                        if let Some(p) = self.playlist_page.as_mut() {
                            if p.on_scrollbar(col, row) {
                                self.browse_drag = true;
                                p.scroll_to_bar(row);
                            } else if let Some(hit) = p.hit(col, row) {
                                action = p.click(hit);
                            }
                        }
                        self.apply_playlist(action);
                        return;
                    }
                    if self.page() == Page::Search {
                        let mut action = browse::Action::None;
                        let p = &mut self.search_page;
                        if p.on_scrollbar(col, row) {
                            self.browse_drag = true;
                            p.scroll_to_bar(row);
                        } else if let Some(hit) = p.hit(col, row) {
                            action = p.click(hit);
                        }
                        self.apply_browse(action);
                        return;
                    }
                    if self.page() == Page::History {
                        let mut action = browse::Action::None;
                        let p = &mut self.history_page;
                        if p.on_scrollbar(col, row) {
                            self.browse_drag = true;
                            p.scroll_to_bar(row);
                        } else if let Some(hit) = p.hit(col, row) {
                            action = p.click(hit);
                        }
                        self.apply_browse(action);
                        return;
                    }
                    if self.page() == Page::Settings {
                        let p = &mut self.settings_page;
                        let mut outcome = None;
                        if p.on_scrollbar(col, row) {
                            self.browse_drag = true;
                            p.scroll_to_bar(row);
                        } else if let Some(hit) = p.hit(col, row) {
                            outcome = p.click(hit);
                        }
                        if let Some(outcome) = outcome {
                            self.apply_outcome(outcome);
                        }
                        return;
                    }
                    if let Page::Album(_) = self.page() {
                        let mut action = browse::Action::None;
                        if let Some(p) = self.album_page.as_mut() {
                            if p.on_scrollbar(col, row) {
                                self.browse_drag = true;
                                p.scroll_to_bar(row);
                            } else if let Some(hit) = p.hit(col, row) {
                                action = p.click(hit);
                            }
                        }
                        self.apply_browse(action);
                        return;
                    }
                    let mut dragging = false;
                    let mut action = browse::Action::None;
                    if let Some(browse) = self.browse_mut() {
                        if browse.on_scrollbar(col, row) {
                            dragging = true;
                            browse.scroll_to_bar(row);
                        } else if let Some(hit) = browse.hit(col, row) {
                            action = browse.click(hit);
                        }
                    }
                    self.browse_drag |= dragging;
                    self.apply_browse(action);
                    return;
                }

                // Library pane click.
                if self.library_active()
                    && self.is_in_rect(event.column, event.row, self.layout.library_area)
                {
                    let area = self.layout.library_area;
                    if event.row == area.y
                        && event.column + (library::FILTER_CONTROL.len() as u16) >= area.x + area.width
                        && let Some(lib) = self.library.as_mut()
                        && lib.filter.is_empty()
                        && !lib.filter_active
                    {
                        lib.start_filter();
                        return;
                    }
                    if let Some((x, y, h)) = self.library_bar()
                        && event.column >= x
                        && event.row >= y
                        && event.row < y + h as u16
                    {
                        self.browse_drag = true;
                        self.library_scroll_to(event.row);
                        return;
                    }
                    let inner_h = self.library_content_height();
                    if let Some(ref mut lib) = self.library {
                        let inner_x = self.layout.library_area.x;
                        let inner_y = self.layout.library_area.y + 1;
                        if event.row >= inner_y && (event.row - inner_y) < inner_h as u16 {
                            let row = (event.row - inner_y) as usize;
                            let col = event.column.saturating_sub(inner_x) as usize;
                            let item_idx = lib.scroll_offset + row;
                            if item_idx < lib.nodes.len() {
                                lib.cursor = item_idx;

                                // Click on arrow area (first ~4 chars) -> expand/collapse.
                                // Click on text -> double-click to enqueue.
                                if col < 4 {
                                    lib.toggle_expand();
                                    self.last_click_idx = None;
                                    self.last_click_time = None;
                                } else {
                                    let now = std::time::Instant::now();
                                    let is_double = self.last_click_idx == Some(item_idx)
                                        && self.last_click_time.is_some_and(|t| {
                                            now.duration_since(t).as_millis() < 400
                                        });
                                    if is_double {
                                        self.last_click_idx = None;
                                        self.last_click_time = None;
                                        let ids = lib.enqueue_all_under_cursor();
                                        if !ids.is_empty() {
                                            self.picker_result = Some((
                                                PickerKind::Track,
                                                ids,
                                                PickerAction::AppendAndPlay,
                                            ));
                                        }
                                    } else {
                                        self.last_click_idx = Some(item_idx);
                                        self.last_click_time = Some(now);
                                    }
                                }
                            }
                        }
                    }
                    return;
                }

                // Click on now-playing art -> zoom.
                if self.art.now_playing_art.cached().is_some()
                    && self.layout.now_playing_art_area.width > 0
                    && self.is_in_rect(event.column, event.row, self.layout.now_playing_art_area)
                {
                    self.mode = Mode::CoverArtZoom;
                    return;
                }

                // Seek bar click — only the first row of the transport area.
                if event.row == self.layout.transport_text_area.y
                    && event.column >= self.layout.transport_text_area.x
                    && event.column
                        < self.layout.transport_text_area.x + self.layout.transport_text_area.width
                    && self.state.track_info().is_some()
                {
                    let click_x = event.column;
                    // The duration the bar was drawn against, not the probed
                    // one: a partial file reports short, and a click would then
                    // land somewhere other than where it was aimed.
                    let dur = self.state.duration_ms();

                    if let Some(pos) = TransportBar::seek_from_click(
                        self.layout.seek_bar_start,
                        self.layout.seek_bar_width,
                        click_x,
                        dur,
                        self.state.seek_ceiling_ms(),
                    ) {
                        self.seek_to(pos);
                    } else if click_x < self.layout.seek_bar_start {
                        // Clicked on the play/pause status icon — toggle.
                        self.toggle_pause();
                    }
                    return;
                }

                // Scrollbar click — rightmost column of queue area (inside border).
                // All thumb math uses 1/8th-cell units for sub-pixel precision.
                let q = self.layout.queue_area;
                let scrollbar_x = q.x + q.width - 1;
                if event.column == scrollbar_x && event.row > q.y && event.row < q.y + q.height {
                    let inner_y = q.y + 1;
                    let visible_height = self.queue_visible_height();
                    let visible = self.visible_queue();
                    let total_lines = super::queue::display_line_count(&visible);

                    if total_lines > visible_height && visible_height > 0 {
                        let vis8 = visible_height * 8;
                        let thumb_8 = (vis8 * visible_height / total_lines).max(8);
                        let max_scroll = total_lines.saturating_sub(visible_height);
                        let track_8 = vis8.saturating_sub(thumb_8);
                        let thumb_top_8 = if max_scroll > 0 && track_8 > 0 {
                            self.queue.scroll_offset * track_8 / max_scroll
                        } else {
                            0
                        };
                        let thumb_bot_8 = thumb_top_8 + thumb_8;
                        let click_row = (event.row - inner_y) as usize;
                        let click_8 = click_row * 8 + 4; // center of clicked cell

                        if click_8 >= thumb_top_8 && click_8 < thumb_bot_8 {
                            // Clicked on thumb — record grab offset in eighths.
                            self.scrollbar_grab_offset = Some((click_8 - thumb_top_8) as u16);
                        } else {
                            // Clicked on track — jump, grab at thumb center.
                            self.scrollbar_grab_offset = Some((thumb_8 / 2) as u16);
                            self.scroll_to_scrollbar_y(event.row);
                        }
                    }
                    return;
                }

                // Queue area click.
                if !self.is_in_rect(event.column, event.row, self.layout.queue_area) {
                    return;
                }

                let visible = self.visible_queue();
                if let Some((first, link)) = queue::QueueView::header_link_at(
                    &visible,
                    self.layout.queue_area,
                    self.queue.scroll_offset,
                    event.column,
                    event.row,
                ) {
                    let db_id = visible.get(first).and_then(|e| e.db_id);
                    self.open_entry_link(db_id, link);
                    return;
                }
                if event.column == queue::menu_button_x(self.layout.queue_area)
                    && let Some(idx) = queue::QueueView::queue_index_at_y(
                        &visible,
                        self.layout.queue_area,
                        self.queue.scroll_offset,
                        event.row,
                    )
                {
                    self.open_queue_menu(idx, event.column, event.row);
                    return;
                }
                let Some(idx) = queue::QueueView::queue_index_at_y(
                    &visible,
                    self.layout.queue_area,
                    self.queue.scroll_offset,
                    event.row,
                ) else {
                    // Check if the click landed on an album header.
                    if let Some((first, last)) = queue::QueueView::album_group_at_y(
                        &visible,
                        self.layout.queue_area,
                        self.queue.scroll_offset,
                        event.row,
                    ) {
                        // Double-click on album header -> play from first track.
                        let now = std::time::Instant::now();
                        let is_double = self.last_click_idx == Some(first)
                            && self
                                .last_click_time
                                .is_some_and(|t| now.duration_since(t).as_millis() < 400);

                        if is_double {
                            self.last_click_idx = None;
                            self.last_click_time = None;
                            self.queue.cursor = first;
                            self.play_at_cursor();
                        } else {
                            self.last_click_idx = Some(first);
                            self.last_click_time = Some(now);

                            // Select all tracks in the album group.
                            self.queue.selected_ids.clear();
                            for i in first..=last {
                                if let Some(entry) = visible.get(i) {
                                    self.queue.selected_ids.insert(entry.id);
                                }
                            }
                            self.queue.cursor = first;
                            if let Some(entry) = visible.get(first) {
                                self.queue.anchor_id = Some(entry.id);
                            }
                            // Start drag so the album group can be reordered.
                            self.queue.drag = Some(DragState {
                                from_index: first,
                                current_y: event.row,
                                multi: true,
                                anchor_offset: 0,
                                last_group_start: Some(first),
                            });
                        }
                    }
                    return;
                };

                // Range-select: Shift or Ctrl (most terminals don't report Shift on mouse).
                let range_select = event.modifiers.contains(KeyModifiers::SHIFT)
                    || event.modifiers.contains(KeyModifiers::CONTROL);
                // Alt/Option for toggle-select (Cmd doesn't reach terminal).
                let toggle = event.modifiers.contains(KeyModifiers::ALT);

                // Mouse editing works in any mode — modality is keyboard-only.
                // Double-click plays; single click selects; drag reorders.
                let now = std::time::Instant::now();
                let is_double_click = self.last_click_idx == Some(idx)
                    && self
                        .last_click_time
                        .is_some_and(|t| now.duration_since(t).as_millis() < 400);

                if is_double_click {
                    // Double-click -> play the track at cursor.
                    self.last_click_idx = None;
                    self.last_click_time = None;
                    self.queue.cursor = idx;
                    self.play_at_cursor();
                } else {
                    self.last_click_idx = Some(idx);
                    self.last_click_time = Some(now);

                    if range_select {
                        self.extend_selection_to(idx);
                    } else if toggle {
                        self.toggle_selection(idx);
                    } else {
                        let id_at_idx = self.queue.vq_cache.entries.get(idx).map(|e| e.id);
                        let already_selected =
                            id_at_idx.is_some_and(|id| self.queue.selected_ids.contains(&id));
                        if !already_selected {
                            // Only deselect others if clicking a NON-selected track.
                            // Clicking an already-selected track preserves the
                            // multi-selection so the user can drag the whole group.
                            self.select_single(idx);
                        }
                    }
                    self.queue.cursor = idx;

                    let multi = self.queue.selected_ids.len() > 1;
                    let anchor_offset = if multi {
                        let mut indices: Vec<usize> = self.selected_indices().into_iter().collect();
                        indices.sort_unstable();
                        let first = indices.first().copied().unwrap_or(idx);
                        idx.saturating_sub(first)
                    } else {
                        0
                    };
                    self.queue.drag = Some(DragState {
                        from_index: idx,
                        current_y: event.row,
                        multi,
                        anchor_offset,
                        last_group_start: if multi {
                            let mut indices: Vec<usize> =
                                self.selected_indices().into_iter().collect();
                            indices.sort_unstable();
                            indices.first().copied()
                        } else {
                            Some(idx)
                        },
                    });
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if self.browse_drag => {
                let row = event.row;
                if self.page() == Page::Playlists {
                    self.playlists_page.scroll_to_bar(row);
                } else if let Page::Playlist(_) = self.page() {
                    if let Some(p) = self.playlist_page.as_mut() {
                        p.scroll_to_bar(row);
                    }
                } else if self.page() == Page::Search {
                    self.search_page.scroll_to_bar(row);
                } else if self.page() == Page::History {
                    self.history_page.scroll_to_bar(row);
                } else if self.page() == Page::Settings {
                    self.settings_page.scroll_to_bar(row);
                } else if let Page::Album(_) = self.page() {
                    if let Some(p) = self.album_page.as_mut() {
                        p.scroll_to_bar(row);
                    }
                } else if self.page() == Page::Artists {
                    self.library_scroll_to(row);
                } else if let Some(browse) = self.browse_mut() {
                    browse.scroll_to_bar(row);
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if self.scrollbar_grab_offset.is_some() => {
                self.scroll_to_scrollbar_y(event.row);
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let drag_info = self
                    .queue
                    .drag
                    .as_ref()
                    .map(|d| (d.from_index, d.multi, d.anchor_offset));
                if let Some((from_index, _multi, anchor_offset)) = drag_info {
                    if let Some(ref mut drag) = self.queue.drag {
                        drag.current_y = event.row;
                    }

                    if self.mode == Mode::QueueEdit
                        && self.is_in_rect(event.column, event.row, self.layout.queue_area)
                    {
                        // Edit mode: drag extends selection (shift-click workaround).
                        let visible = self.visible_queue();
                        if let Some(idx) = queue::QueueView::queue_index_at_y(
                            &visible,
                            self.layout.queue_area,
                            self.queue.scroll_offset,
                            event.row,
                        ) {
                            self.extend_selection_to(idx);
                            self.queue.cursor = idx;
                        }
                    } else if self.mode != Mode::QueueEdit
                        && self.is_in_rect(event.column, event.row, self.layout.queue_area)
                    {
                        // Normal mode: live reorder — move track(s) as mouse crosses rows.
                        let visible = self.visible_queue();
                        if let Some(to_idx) = queue::QueueView::queue_index_at_y(
                            &visible,
                            self.layout.queue_area,
                            self.queue.scroll_offset,
                            event.row,
                        ) {
                            if self.queue.selected_ids.len() > 1 {
                                // Multi-drag: compute desired group start from anchor offset
                                // so the clicked item stays under the mouse cursor.
                                let desired_start = to_idx.saturating_sub(anchor_offset);
                                let last_start =
                                    self.queue.drag.as_ref().and_then(|d| d.last_group_start);

                                if Some(desired_start) != last_start {
                                    if !self.drag_undo_active {
                                        self.tx.send(PlayerCommand::BeginUndoBatch).ok();
                                        self.drag_undo_active = true;
                                    }

                                    // Compute target index for send_move_selected.
                                    let mut indices: Vec<usize> =
                                        self.selected_indices().into_iter().collect();
                                    indices.sort_unstable();
                                    let count = indices.len();
                                    let first = indices.first().copied().unwrap_or(0);

                                    let target = if desired_start > first {
                                        // Moving down: place after this index.
                                        (desired_start + count - 1)
                                            .min(visible.len().saturating_sub(1))
                                    } else {
                                        // Moving up: place at this index.
                                        desired_start
                                    };

                                    self.send_move_selected(target);
                                    if let Some(ref mut drag) = self.queue.drag {
                                        drag.last_group_start = Some(desired_start);
                                    }
                                }
                            } else {
                                // Single-track drag.
                                if to_idx != from_index {
                                    if !self.drag_undo_active {
                                        self.tx.send(PlayerCommand::BeginUndoBatch).ok();
                                        self.drag_undo_active = true;
                                    }
                                    // Capture the dragged track's ID before the move —
                                    // vq_cache won't refresh until next frame, so
                                    // select_single(to_idx) would grab the displaced track.
                                    let dragged_id =
                                        self.queue.vq_cache.entries.get(from_index).map(|e| e.id);
                                    self.send_move(from_index, to_idx);
                                    self.queue.cursor = to_idx;
                                    self.queue.selected_ids.clear();
                                    if let Some(id) = dragged_id {
                                        self.queue.selected_ids.insert(id);
                                        self.queue.anchor_id = Some(id);
                                    }
                                    if let Some(ref mut drag) = self.queue.drag {
                                        drag.from_index = to_idx;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                // Close undo batch if a drag reorder was active.
                if self.drag_undo_active {
                    self.tx.send(PlayerCommand::EndUndoBatch).ok();
                    self.drag_undo_active = false;
                }
                // Clear drag state — reorder already happened live during drag.
                self.queue.drag.take();
                self.scrollbar_grab_offset = None;
                self.browse_drag = false;
            }
            MouseEventKind::Down(MouseButton::Right)
                if self.page() != Page::Queue
                    && self.is_in_rect(event.column, event.row, self.layout.browse_area) => {
                    // Right-click on a record or track -> add it to the queue.
                    let (col, row) = (event.column, event.row);
                    if let Some(browse) = self.browse_mut()
                        && let Some(browse::Hit::Item(idx)) = browse.hit(col, row)
                    {
                        let action = browse.append_at(idx);
                        self.apply_browse(action);
                        self.status_message =
                            Some(("added to the queue".into(), std::time::Instant::now()));
                    }
                }
            MouseEventKind::Down(MouseButton::Right)
                // Right-click on queue item -> context menu at click position.
                if self.is_in_rect(event.column, event.row, self.layout.queue_area) => {
                    let visible = self.visible_queue();
                    if let Some(idx) = queue::QueueView::queue_index_at_y(
                        &visible,
                        self.layout.queue_area,
                        self.queue.scroll_offset,
                        event.row,
                    ) {
                        // Select the clicked item if not already selected.
                        let id_at_idx = self.queue.vq_cache.entries.get(idx).map(|e| e.id);
                        if !id_at_idx.is_some_and(|id| self.queue.selected_ids.contains(&id)) {
                            self.select_single(idx);
                        }
                        self.queue.cursor = idx;


                        let is_fav = visible
                            .get(idx)
                            .is_some_and(|e| e.db_id.is_some_and(|id| self.favourites.contains(&id)));
                        let fav_label = if is_fav { "Unfavourite" } else { "Favourite" };
                        let actions = self.build_context_actions(fav_label);
                        self.context_menu = Some(ContextMenuState { actions, cursor: 0 });
                        self.push_mode(Mode::ContextMenu);
                        // Store click position for positioned rendering.
                        self.hover.column = event.column;
                        self.hover.row = event.row;
                    } else if let Some((first, last)) = queue::QueueView::album_group_at_y(
                        &visible,
                        self.layout.queue_area,
                        self.queue.scroll_offset,
                        event.row,
                    ) {
                        // Right-click on album header -> select the whole group and open context menu.
                        self.queue.selected_ids.clear();
                        for i in first..=last {
                            if let Some(entry) = visible.get(i) {
                                self.queue.selected_ids.insert(entry.id);
                            }
                        }
                        self.queue.cursor = first;
                        if let Some(entry) = visible.get(first) {
                            self.queue.anchor_id = Some(entry.id);
                        }

                        // Favourite label: true only if ALL tracks in the group are favourited.
                        let all_fav = (first..=last).all(|i| {
                            visible
                                .get(i)
                                .is_some_and(|e| e.db_id.is_some_and(|id| self.favourites.contains(&id)))
                        });
                        let fav_label = if all_fav { "Unfavourite" } else { "Favourite" };
                        let actions = self.build_context_actions(fav_label);
                        self.context_menu = Some(ContextMenuState { actions, cursor: 0 });
                        self.push_mode(Mode::ContextMenu);
                        self.hover.column = event.column;
                        self.hover.row = event.row;
                    }
                }
            MouseEventKind::ScrollUp => {
                if let Mode::Picker(_) = &self.mode {
                    if let Some(ref mut picker) = self.picker {
                        picker.move_up();
                    }
                } else if self.page() != Page::Queue
                    && self.is_in_rect(event.column, event.row, self.layout.browse_area)
                {
                    if self.page() == Page::Playlists {
                        self.playlists_page.scroll_by(-3);
                    } else if let Page::Playlist(_) = self.page() {
                        if let Some(p) = self.playlist_page.as_mut() {
                            p.scroll_by(-3);
                        }
                    } else if self.page() == Page::Search {
                        self.search_page.scroll_by(-3);
                    } else if self.page() == Page::History {
                        self.history_page.scroll_by(-3);
                    } else if self.page() == Page::Settings {
                        self.settings_page.scroll_by(-3);
                    } else if let Page::Album(_) = self.page() {
                        if let Some(p) = self.album_page.as_mut() {
                            p.scroll_by(-3);
                        }
                    } else if let Some(browse) = self.browse_mut() {
                        browse.scroll_by(-1);
                    }
                } else if self.library_active()
                    && self.is_in_rect(event.column, event.row, self.layout.library_area)
                {
                    if let Some(ref mut lib) = self.library {
                        lib.scroll_offset = lib.scroll_offset.saturating_sub(3);
                    }
                } else if self.page() != Page::Queue {
                } else {
                    self.queue.scroll_offset = self.queue.scroll_offset.saturating_sub(1);
                }
            }
            MouseEventKind::ScrollDown => {
                if let Mode::Picker(_) = &self.mode {
                    if let Some(ref mut picker) = self.picker {
                        picker.move_down();
                    }
                } else if self.page() != Page::Queue
                    && self.is_in_rect(event.column, event.row, self.layout.browse_area)
                {
                    if self.page() == Page::Playlists {
                        self.playlists_page.scroll_by(3);
                    } else if let Page::Playlist(_) = self.page() {
                        if let Some(p) = self.playlist_page.as_mut() {
                            p.scroll_by(3);
                        }
                    } else if self.page() == Page::Search {
                        self.search_page.scroll_by(3);
                    } else if self.page() == Page::History {
                        self.history_page.scroll_by(3);
                    } else if self.page() == Page::Settings {
                        self.settings_page.scroll_by(3);
                    } else if let Page::Album(_) = self.page() {
                        if let Some(p) = self.album_page.as_mut() {
                            p.scroll_by(3);
                        }
                    } else if let Some(browse) = self.browse_mut() {
                        browse.scroll_by(1);
                    }
                } else if self.library_active()
                    && self.is_in_rect(event.column, event.row, self.layout.library_area)
                {
                    let h = self.library_content_height();
                    if let Some(ref mut lib) = self.library {
                        let max = lib.nodes.len().saturating_sub(h);
                        lib.scroll_offset = (lib.scroll_offset + 3).min(max);
                    }
                } else if self.page() != Page::Queue {
                } else {
                    let visible = self.visible_queue();
                    let total_display = super::queue::display_line_count(&visible);
                    let visible_height = self.queue_visible_height();
                    let max_scroll = total_display.saturating_sub(visible_height);
                    self.queue.scroll_offset = (self.queue.scroll_offset + 1).min(max_scroll);
                }
            }
            MouseEventKind::Moved => {
                self.hover.column = event.column;
                self.hover.row = event.row;
                self.update_hover_zone();
            }
            _ => {}
        }
    }

    /// Compute the hover zone from current mouse position and layout rects.
    /// The menu of actions for the queue row `idx`, opened at `col`, `row`.
    /// A right click opens it, and so does the row's own button.
    fn open_queue_menu(&mut self, idx: usize, col: u16, row: u16) {
        let visible = self.visible_queue();
        let id_at_idx = self.queue.vq_cache.entries.get(idx).map(|e| e.id);
        if !id_at_idx.is_some_and(|id| self.queue.selected_ids.contains(&id)) {
            self.select_single(idx);
        }
        self.queue.cursor = idx;
        let is_fav = visible
            .get(idx)
            .is_some_and(|e| e.db_id.is_some_and(|id| self.favourites.contains(&id)));
        let fav_label = if is_fav { "Unfavourite" } else { "Favourite" };
        let actions = self.build_context_actions(fav_label);
        self.context_menu = Some(ContextMenuState { actions, cursor: 0 });
        self.push_mode(Mode::ContextMenu);
        self.hover.column = col;
        self.hover.row = row;
    }

    /// Opens the record or the artist of the library track `db_id`.
    pub fn open_entry_link(&mut self, db_id: Option<i64>, link: queue::HeaderLink) {
        let found = db_id.and_then(|id| {
            let db = koan_core::db::pool::shared().get().ok()?;
            let track = koan_core::db::queries::get_track_row(&db.conn, id)
                .ok()
                .flatten()?;
            let album = koan_core::db::queries::get_album(&db.conn, track.album_id?)
                .ok()
                .flatten()?;
            Some(match link {
                queue::HeaderLink::Album => Page::Album(album.id),
                queue::HeaderLink::Artist => Page::Artist(album.artist_id),
            })
        });
        match found {
            Some(page) => self.go_page(page),
            None => {
                self.status_message = Some((
                    "that track is not in the library".into(),
                    std::time::Instant::now(),
                ));
            }
        }
    }

    fn nav_target_at(&self, col: u16, row: u16) -> Option<NavTarget> {
        self.layout
            .nav_hits
            .iter()
            .find(|(r, _)| col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height)
            .map(|(_, t)| *t)
    }

    /// Whether the artists tree is the page in front.
    pub fn library_active(&self) -> bool {
        self.mode == Mode::Normal && self.page() == Page::Artists && self.library.is_some()
    }

    fn update_hover_zone(&mut self) {
        let col = self.hover.column;
        let row = self.hover.row;

        // Seek bar (first row of transport text area).
        if row == self.layout.transport_text_area.y
            && col >= self.layout.seek_bar_start
            && col < self.layout.seek_bar_start + self.layout.seek_bar_width
        {
            self.hover.zone = HoverZone::SeekBar;
            return;
        }

        // Transport art area.
        if self.layout.now_playing_art_area.width > 0
            && self.is_in_rect(col, row, self.layout.now_playing_art_area)
        {
            self.hover.zone = HoverZone::TransportArt;
            return;
        }

        // Transport text area.
        if self.is_in_rect(col, row, self.layout.transport_text_area) {
            self.hover.zone = HoverZone::TransportText;
            return;
        }

        // Queue scrollbar (rightmost column).
        let q = self.layout.queue_area;
        if q.width > 0 && col == q.x + q.width - 1 && row > q.y && row < q.y + q.height {
            self.hover.zone = HoverZone::ScrollbarQueue;
            return;
        }

        // Queue item.
        if self.is_in_rect(col, row, self.layout.queue_area) {
            let visible = self.visible_queue();
            if let Some(idx) = queue::QueueView::queue_index_at_y(
                &visible,
                self.layout.queue_area,
                self.queue.scroll_offset,
                row,
            ) {
                self.hover.zone = HoverZone::QueueItem(idx);
                return;
            }
        }

        if let Some(target) = self.nav_target_at(col, row) {
            self.hover.zone = HoverZone::Nav(target);
            return;
        }

        if self.page() != Page::Queue && self.is_in_rect(col, row, self.layout.browse_area) {
            if self.page() == Page::Playlists {
                if let Some(hit) = self.playlists_page.hit(col, row) {
                    self.hover.zone = HoverZone::Playlists(hit);
                    return;
                }
            } else if let Page::Playlist(_) = self.page() {
                if let Some(hit) = self.playlist_page.as_ref().and_then(|p| p.hit(col, row)) {
                    self.hover.zone = HoverZone::Playlist(hit);
                    return;
                }
            } else if self.page() == Page::Search {
                if let Some(hit) = self.search_page.hit(col, row) {
                    self.hover.zone = HoverZone::Search(hit);
                    return;
                }
            } else if self.page() == Page::History {
                if let Some(hit) = self.history_page.hit(col, row) {
                    self.hover.zone = HoverZone::History(hit);
                    return;
                }
            } else if self.page() == Page::Settings {
                if let Some(hit) = self.settings_page.hit(col, row) {
                    self.hover.zone = HoverZone::Settings(hit);
                    return;
                }
            } else if let Page::Album(_) = self.page() {
                if let Some(hit) = self.album_page.as_ref().and_then(|p| p.hit(col, row)) {
                    self.hover.zone = HoverZone::Album(hit);
                    return;
                }
            } else {
                let hit = match self.page() {
                    Page::Albums => self.albums_page.hit(col, row),
                    Page::Favourites => self.favourites_page.hit(col, row),
                    Page::Recent => self.recent_page.hit(col, row),
                    Page::Artist(_) => self.artist_page.hit(col, row),
                    _ => None,
                };
                if let Some(hit) = hit {
                    self.hover.zone = HoverZone::Browse(hit);
                    return;
                }
            }
        }

        // Library item.
        if self.library_active() && self.is_in_rect(col, row, self.layout.library_area) {
            let inner_y = self.layout.library_area.y + 1;
            let inner_h = self.library_content_height();
            if row >= inner_y
                && (row - inner_y) < inner_h as u16
                && let Some(ref lib) = self.library
            {
                let item_idx = lib.scroll_offset + (row - inner_y) as usize;
                if item_idx < lib.nodes.len() {
                    self.hover.zone = HoverZone::LibraryItem(item_idx);
                    return;
                }
            }
        }

        self.hover.zone = HoverZone::None;
    }

    // --- Selection helpers ---

    /// Derive selected indices from selected IDs for the current visible queue.
    pub fn selected_indices(&self) -> HashSet<usize> {
        let visible = &self.queue.vq_cache.entries;
        visible
            .iter()
            .enumerate()
            .filter(|(_, e)| self.queue.selected_ids.contains(&e.id))
            .map(|(i, _)| i)
            .collect()
    }

    /// Get the index of the anchor in the current visible queue.
    fn anchor_index(&self) -> Option<usize> {
        let anchor = self.queue.anchor_id?;
        let visible = &self.queue.vq_cache.entries;
        visible.iter().position(|e| e.id == anchor)
    }

    /// Plain click / arrow: clear selection, select one track, set anchor.
    fn select_single(&mut self, idx: usize) {
        let id = self.queue.vq_cache.entries.get(idx).map(|e| e.id);
        self.queue.selected_ids.clear();
        if let Some(id) = id {
            self.queue.selected_ids.insert(id);
            self.queue.anchor_id = Some(id);
        }
    }

    /// Shift-click/arrow: select range from anchor to idx (inclusive).
    fn extend_selection_to(&mut self, idx: usize) {
        let anchor = self.anchor_index().unwrap_or(self.queue.cursor);
        let lo = anchor.min(idx);
        let hi = anchor.max(idx);
        // Shift replaces the selection with the range from the anchor.
        self.queue.selected_ids.clear();
        let visible = &self.queue.vq_cache.entries;
        for i in lo..=hi {
            if let Some(e) = visible.get(i) {
                self.queue.selected_ids.insert(e.id);
            }
        }
        // Keep anchor where it was (only set if not already set).
        if self.queue.anchor_id.is_none()
            && let Some(e) = visible.get(anchor)
        {
            self.queue.anchor_id = Some(e.id);
        }
    }

    /// Select all visible queue entries, anchoring to the first.
    fn select_all(&mut self) {
        let visible = &self.queue.vq_cache.entries;
        self.queue.selected_ids.clear();
        let first_id = visible.first().map(|e| e.id);
        for e in visible {
            self.queue.selected_ids.insert(e.id);
        }
        self.queue.anchor_id = first_id;
    }

    /// Alt-click: toggle one track in/out of selection set.
    fn toggle_selection(&mut self, idx: usize) {
        if let Some(e) = self.queue.vq_cache.entries.get(idx) {
            let id = e.id;
            if self.queue.selected_ids.contains(&id) {
                self.queue.selected_ids.remove(&id);
            } else {
                self.queue.selected_ids.insert(id);
            }
            // Move anchor to last toggled.
            self.queue.anchor_id = Some(id);
        }
    }

    /// Play the track at the current cursor position (Enter / double-click).
    /// If the track is Pending (needs download), triggers a priority download
    /// so playback starts via stream-when-ready.
    fn play_at_cursor(&mut self) {
        let idx = self.queue.cursor;
        let visible = self.visible_queue();
        if let Some(entry) = visible.get(idx)
            && entry.status != QueueEntryStatus::Playing
        {
            // A track still to be fetched jumps the download queue when the
            // cursor reaches it.
            self.tx.send(PlayerCommand::Play(entry.id)).ok();
        }
    }

    /// Delete all selected tracks.
    fn delete_selected(&mut self) {
        let ids: Vec<_> = if self.queue.selected_ids.is_empty() {
            // Nothing selected — delete the track under cursor.
            let visible = self.visible_queue();
            visible
                .get(self.queue.cursor)
                .map(|e| e.id)
                .into_iter()
                .collect()
        } else {
            self.queue.selected_ids.iter().copied().collect()
        };

        if !ids.is_empty() {
            self.tx
                .send(PlayerCommand::RemoveFromPlaylistBatch(ids))
                .ok();
        }

        self.queue.selected_ids.clear();
        self.clamp_queue_cursor();
    }

    /// Move all selected tracks down by one position.
    fn move_selected_down(&mut self) {
        let visible_len = self.visible_queue().len();

        // Single item: move the track under cursor.
        if self.queue.selected_ids.len() <= 1 {
            if self.queue.cursor + 1 < visible_len {
                self.send_move(self.queue.cursor, self.queue.cursor + 1);
                self.queue.cursor += 1;
                self.select_single(self.queue.cursor);
                self.update_scroll();
            }
            return;
        }

        let mut indices: Vec<usize> = self.selected_indices().into_iter().collect();
        indices.sort_unstable();

        let max_idx = indices.last().copied().unwrap_or(0);
        if max_idx + 1 >= visible_len {
            return;
        }
        let min_idx = indices.first().copied().unwrap_or(0);

        // Swap the item BELOW the group to ABOVE it — single atomic move.
        self.send_move(max_idx + 1, min_idx);

        // IDs are stable — selection follows automatically. Just update cursor.
        self.queue.cursor += 1;
        self.update_scroll();
    }

    /// Move all selected tracks up by one position.
    fn move_selected_up(&mut self) {
        // Single item: move the track under cursor.
        if self.queue.selected_ids.len() <= 1 {
            if self.queue.cursor > 0 {
                self.send_move(self.queue.cursor, self.queue.cursor - 1);
                self.queue.cursor -= 1;
                self.select_single(self.queue.cursor);
                self.update_scroll();
            }
            return;
        }

        let mut indices: Vec<usize> = self.selected_indices().into_iter().collect();
        indices.sort_unstable();

        let min_idx = indices.first().copied().unwrap_or(0);
        if min_idx == 0 {
            return;
        }
        let max_idx = indices.last().copied().unwrap_or(0);

        // Swap the item ABOVE the group to BELOW it — single atomic move.
        self.send_move(min_idx - 1, max_idx);

        // IDs are stable — selection follows automatically. Just update cursor.
        self.queue.cursor -= 1;
        self.update_scroll();
    }

    /// Send a batch move for all selected tracks to a target position.
    /// Used for multi-track drag.
    fn send_move_selected(&mut self, target_idx: usize) {
        let mut indices: Vec<usize> = self.selected_indices().into_iter().collect();
        indices.sort_unstable();

        let visible = self.visible_queue();
        let ids: Vec<_> = indices
            .iter()
            .filter_map(|&i| visible.get(i).map(|e| e.id))
            .collect();
        let Some(target_entry) = visible.get(target_idx) else {
            return;
        };
        let target_id = target_entry.id;

        // Use MoveItemsInPlaylist to move the whole group atomically.
        let moving_down = target_idx > indices.first().copied().unwrap_or(0);
        self.tx
            .send(PlayerCommand::MoveItemsInPlaylist {
                ids,
                target: target_id,
                after: moving_down,
            })
            .ok();

        // IDs are stable — selected_ids already contains the right IDs.
        // Just update the cursor to the new visual position.
        let count = indices.len();
        let new_start = if moving_down {
            target_idx + 1 - count
        } else {
            target_idx
        };
        self.queue.cursor = if moving_down {
            new_start + count - 1
        } else {
            new_start
        };
    }

    /// Next or previous, at most once per 150 ms. A held key repeats faster
    /// than anyone means to skip, and the player acts on every command it is
    /// sent.
    fn skip(&mut self, cmd: PlayerCommand) {
        let now = std::time::Instant::now();
        if self
            .last_skip
            .is_some_and(|at| now.duration_since(at) < std::time::Duration::from_millis(150))
        {
            return;
        }
        self.last_skip = Some(now);
        self.tx.send(cmd).ok();
    }

    fn toggle_pause(&self) {
        let cmd = if self.state.wants_to_play() {
            PlayerCommand::Pause
        } else {
            PlayerCommand::Resume
        };
        self.tx.send(cmd).ok();
    }

    /// Send a move command for a visible queue index pair.
    fn send_move(&self, from_visible: usize, to_visible: usize) {
        let visible = self.visible_queue();
        let Some(from_entry) = visible.get(from_visible) else {
            return;
        };
        let Some(to_entry) = visible.get(to_visible) else {
            return;
        };

        let after = to_visible > from_visible;
        self.tx
            .send(PlayerCommand::MoveInPlaylist {
                id: from_entry.id,
                target: to_entry.id,
                after,
            })
            .ok();
    }

    fn handle_library_key(&mut self, key: KeyEvent) {
        // When filter input is focused, route keys there first.
        if self.library.as_ref().is_some_and(|lib| lib.filter_active) {
            self.handle_library_filter_key(key);
            return;
        }

        if key.code == KeyCode::Esc {
            // A filter is cleared first; otherwise the page is left.
            if let Some(lib) = self.library.as_mut().filter(|l| !l.filter.is_empty()) {
                lib.clear_filter();
            } else {
                self.leave_page();
            }
            return;
        }
        if !self.handle_library_browse_key(key) {
            self.handle_transport_key(key);
        }
    }

    fn handle_library_filter_key(&mut self, key: KeyEvent) {
        let Some(ref mut lib) = self.library else {
            return;
        };
        match key.code {
            KeyCode::Esc => {
                lib.clear_filter();
            }
            KeyCode::Enter => {
                lib.stop_filter();
            }
            KeyCode::Backspace => {
                if lib.filter.is_empty() {
                    lib.stop_filter();
                } else {
                    lib.filter_backspace();
                }
            }
            KeyCode::Char(c) => {
                lib.filter_type_char(c);
            }
            _ => {}
        }
    }

    fn handle_library_browse_key(&mut self, key: KeyEvent) -> bool {
        let Some(ref mut lib) = self.library else {
            return false;
        };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => lib.move_up(),
            KeyCode::Down | KeyCode::Char('j') => lib.move_down(),
            KeyCode::PageUp => lib.page_up(10),
            KeyCode::PageDown => lib.page_down(10),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                lib.page_up(10);
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                lib.page_down(10);
            }
            KeyCode::Home | KeyCode::Char('g') => lib.jump_to_start(),
            KeyCode::End | KeyCode::Char('G') => lib.jump_to_end(),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                if let Some(ids) = lib.expand_or_enter() {
                    self.picker_result =
                        Some((PickerKind::Track, ids, PickerAction::AppendAndPlay));
                }
            }
            KeyCode::Left | KeyCode::Backspace | KeyCode::Char('h') => {
                lib.collapse_or_parent();
            }
            KeyCode::Char('a') => {
                let ids = lib.enqueue_all_under_cursor();
                if !ids.is_empty() {
                    self.picker_result = Some((PickerKind::Track, ids, PickerAction::Append));
                }
            }
            KeyCode::Char('w') => {
                let ids = lib.enqueue_all_under_cursor();
                self.open_chooser(ids);
            }
            KeyCode::Char('N') => {
                let ids = lib.enqueue_all_under_cursor();
                if !ids.is_empty() {
                    self.picker_result = Some((PickerKind::Track, ids, PickerAction::PlayNext));
                }
            }
            KeyCode::Char('f') | KeyCode::Char('/') => {
                lib.start_filter();
            }
            KeyCode::Char('A') => {
                if let Some(id) = lib.artist_target() {
                    self.go_page(Page::Artist(id));
                }
            }
            KeyCode::Char('O') => {
                if let Some(id) = lib.album_target() {
                    self.go_page(Page::Album(id));
                }
            }
            _ => return false,
        }
        true
    }

    pub fn open_library(&mut self) {
        self.go_page(Page::Artists);
    }

    fn open_picker(&mut self, kind: PickerKind) {
        self.mode = Mode::Picker(kind);
    }

    fn open_queue_jump(&mut self) {
        let visible = self.visible_queue();
        if visible.is_empty() {
            return;
        }
        let items: Vec<super::picker::PickerItem> = visible
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let has_artist = !e.artist.is_empty() && e.artist != e.album_artist;
                let display = if has_artist {
                    format!("{} \u{2014} {}", e.artist, e.title)
                } else {
                    e.title.clone()
                };
                let parts = if has_artist {
                    vec![
                        (e.artist.clone(), PickerPartKind::Artist),
                        (" \u{2014} ".into(), PickerPartKind::Separator),
                        (e.title.clone(), PickerPartKind::Title),
                    ]
                } else {
                    vec![(e.title.clone(), PickerPartKind::Title)]
                };
                let match_text = format!("{} {} {} {}", e.artist, e.album_artist, e.album, e.title);
                super::picker::PickerItem {
                    id: i as i64,
                    display,
                    match_text,
                    parts,
                }
            })
            .collect();
        self.picker = Some(PickerState::new(PickerKind::QueueJump, items, false));
        self.mode = Mode::Picker(PickerKind::QueueJump);
    }

    /// Move cursor up by one page.
    fn page_up(&mut self, extend: bool) {
        let page_size = self.queue_visible_height().max(1);
        self.queue.cursor = self.queue.cursor.saturating_sub(page_size);
        if extend {
            self.extend_selection_to(self.queue.cursor);
        } else {
            self.select_single(self.queue.cursor);
        }
        self.update_scroll();
    }

    /// Move cursor down by one page.
    fn page_down(&mut self, extend: bool) {
        let visible_len = self.visible_queue().len();
        let page_size = self.queue_visible_height().max(1);
        if visible_len > 0 {
            self.queue.cursor = (self.queue.cursor + page_size).min(visible_len - 1);
        }
        if extend {
            self.extend_selection_to(self.queue.cursor);
        } else {
            self.select_single(self.queue.cursor);
        }
        self.update_scroll();
    }

    /// Jump cursor to start of queue.
    fn jump_to_start(&mut self, extend: bool) {
        self.queue.cursor = 0;
        if extend {
            self.extend_selection_to(self.queue.cursor);
        } else {
            self.select_single(self.queue.cursor);
        }
        self.update_scroll();
    }

    /// Jump cursor to end of queue.
    fn jump_to_end(&mut self, extend: bool) {
        let visible_len = self.visible_queue().len();
        if visible_len > 0 {
            self.queue.cursor = visible_len - 1;
        }
        if extend {
            self.extend_selection_to(self.queue.cursor);
        } else {
            self.select_single(self.queue.cursor);
        }
        self.update_scroll();
    }

    /// Rows of library content on screen. The filter input owns the bottom
    /// row while it has focus.
    pub(super) fn library_content_height(&self) -> usize {
        let inner = self.layout.library_area.height.saturating_sub(1) as usize;
        let filter_rows = self.library.as_ref().is_some_and(|lib| lib.filter_active) as usize;
        inner.saturating_sub(filter_rows)
    }

    /// Rows of queue content on screen. `Borders::TOP` eats the first row.
    ///
    /// Rendering, scrolling and every hit-test read this — they drift apart the
    /// moment there is more than one definition.
    pub(super) fn queue_visible_height(&self) -> usize {
        self.layout.queue_area.height.saturating_sub(1) as usize
    }

    /// The Artists tree's scrollbar, as a column and the rows it spans, when
    /// the tree is longer than the pane.
    fn library_bar(&self) -> Option<(u16, u16, usize)> {
        let lib = self.library.as_ref()?;
        let area = self.layout.library_area;
        let h = self.library_content_height();
        (lib.nodes.len() > h && area.width > 6 && h > 0)
            .then(|| (area.x + area.width - 3, area.y + 1, h))
    }

    fn library_scroll_to(&mut self, row: u16) {
        let Some((_, y, h)) = self.library_bar() else {
            return;
        };
        if let Some(lib) = self.library.as_mut() {
            let max = lib.nodes.len().saturating_sub(h);
            let frac = row.saturating_sub(y) as f64 / h.saturating_sub(1).max(1) as f64;
            lib.scroll_offset = ((frac.clamp(0.0, 1.0) * max as f64).round() as usize).min(max);
        }
    }

    fn update_scroll(&mut self) {
        let visible = self.visible_queue();
        let visible_height = self.queue_visible_height();
        self.queue.scroll_offset = queue::scroll_for_cursor(
            &visible,
            self.queue.cursor,
            self.queue.scroll_offset,
            visible_height,
        );
    }

    /// Scroll the queue based on a click/drag position on the scrollbar.
    /// Uses the same sub-pixel (1/8th-cell) thumb math as queue.rs rendering.
    fn scroll_to_scrollbar_y(&mut self, y: u16) {
        let q = self.layout.queue_area;
        let inner_y = q.y + 1;
        let visible_height = self.queue_visible_height();
        if visible_height == 0 {
            return;
        }

        let visible = self.visible_queue();
        let total_lines = super::queue::display_line_count(&visible);
        if total_lines <= visible_height {
            return;
        }

        let vis8 = visible_height * 8;
        let thumb_8 = (vis8 * visible_height / total_lines).max(8);
        let max_scroll = total_lines.saturating_sub(visible_height);
        let track_8 = vis8.saturating_sub(thumb_8);
        if track_8 == 0 {
            return;
        }

        let grab_offset_8 = self.scrollbar_grab_offset.unwrap_or(0) as usize;
        // Mouse Y is in whole cells — convert to eighths (center of cell).
        let mouse_8 = (y.saturating_sub(inner_y) as usize) * 8 + 4;
        let desired_thumb_top_8 = mouse_8.saturating_sub(grab_offset_8);
        let clamped = desired_thumb_top_8.min(track_8);
        self.queue.scroll_offset = (clamped * max_scroll / track_8).min(max_scroll);
    }

    fn is_in_rect(&self, x: u16, y: u16, rect: ratatui::layout::Rect) -> bool {
        x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
    }

    /// Get the drop indicator index — where external drops would insert.
    /// Only active when mouse is over queue and no internal drag is happening.
    pub fn drop_indicator_index(&self) -> Option<usize> {
        if self.queue.drag.is_some() {
            return None; // internal drag takes precedence
        }
        let row = self.last_mouse_row?;
        let visible = self.visible_queue();
        queue::QueueView::queue_index_at_y(
            &visible,
            self.layout.queue_area,
            self.queue.scroll_offset,
            row,
        )
    }

    /// Get the QueueItemId at the drop indicator position (for InsertInPlaylist).
    pub fn drop_target_queue_id(&self) -> Option<koan_core::player::state::QueueItemId> {
        let idx = self.drop_indicator_index()?;
        self.queue.vq_cache.entries.get(idx).map(|e| e.id)
    }

    /// Clamp the queue cursor to valid bounds. Call after any operation that
    /// may shrink the queue (remove, clear, reorder, external playlist change).
    fn clamp_queue_cursor(&mut self) {
        let len = self.queue.vq_cache.entries.len();
        if len == 0 {
            self.queue.cursor = 0;
        } else if self.queue.cursor >= len {
            self.queue.cursor = len - 1;
        }
    }

    /// Refresh the cached visible queue snapshot from shared state.
    /// Call once per frame before any queue-related reads.
    pub fn refresh_visible_queue(&mut self) {
        let v = self.state.playlist_version();
        let mutated = v != self.queue.vq_version;
        // Download progress lives in an atomic the download thread writes
        // directly, so it moves without the version moving. A queue with
        // something in flight re-derives every frame; nothing else does.
        let downloading = self
            .queue
            .vq_cache
            .entries
            .iter()
            .any(|e| e.download_progress.is_some());
        if !mutated && !downloading {
            return;
        }

        self.queue.vq_cache = self.state.derive_visible_queue();
        if mutated {
            self.queue.vq_version = v;

            self.clamp_queue_cursor();
            self.close_stale_track_info();
            self.note_new_failures();
        }
    }

    /// Say why a track cannot play. Without it, a queue nothing could fetch
    /// looks exactly like a queue still loading.
    fn note_new_failures(&mut self) {
        let mut reasons = self
            .queue
            .vq_cache
            .entries
            .iter()
            // Failed only: an entry waiting for the server carries why, too,
            // and will play once it answers.
            .filter(|e| e.status == QueueEntryStatus::Failed)
            .filter_map(|e| e.error.as_deref())
            .peekable();
        if reasons.peek().is_none() {
            self.noted_failures.clear();
            return;
        }
        let Some(fresh) = reasons
            .find(|r| !self.noted_failures.contains(*r))
            .map(str::to_owned)
        else {
            return;
        };
        self.status_message = Some((format!("can't play — {fresh}"), std::time::Instant::now()));
        self.noted_failures.insert(fresh);
    }

    /// Seek, clamped to what the player will accept.
    ///
    /// A track still arriving is reachable only as far as its bytes go, and one
    /// whose container cannot state a duration until the last of them lands —
    /// Ogg — is reachable nowhere at all. Sent unclamped, every seek into that
    /// track is a seek to zero, so it says why nothing happened instead.
    fn seek_to(&mut self, target_ms: u64) {
        if self.state.track_info().is_none() {
            return;
        }
        let seekable = self.state.seekable_ms();
        if seekable == 0 {
            let so_far = self
                .state
                .current_download_fraction()
                .map(|f| format!(" — {}% so far", (f * 100.0) as u32))
                .unwrap_or_default();
            self.status_message = Some((
                format!("still downloading{so_far} — seekable once it lands"),
                std::time::Instant::now(),
            ));
            return;
        }
        self.tx
            .send(PlayerCommand::Seek(target_ms.min(seekable)))
            .ok();
    }

    /// The track info modal renders nothing once its track leaves the queue,
    /// and would swallow every keystroke until the user guessed Esc.
    pub(super) fn close_stale_track_info(&mut self) {
        if let Mode::TrackInfo(id) = self.mode
            && !self.queue.vq_cache.entries.iter().any(|e| e.id == id)
        {
            self.mode = Mode::Normal;
            self.art.cover_art.clear();
        }
    }

    pub fn visible_queue(&self) -> Vec<QueueEntry> {
        self.queue.vq_cache.entries.clone()
    }
}

/// Copy text to the system clipboard. Tries platform-native tools first.
fn copy_to_clipboard(text: &str) {
    use std::io::Write;
    use std::process::{Command, Stdio};

    // macOS
    if let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn() {
        if let Some(ref mut stdin) = child.stdin {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
        return;
    }
    // Linux: xclip
    if let Ok(mut child) = Command::new("xclip")
        .args(["-selection", "clipboard"])
        .stdin(Stdio::piped())
        .spawn()
    {
        if let Some(ref mut stdin) = child.stdin {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
        return;
    }
    // Linux: xsel
    if let Ok(mut child) = Command::new("xsel")
        .args(["--clipboard", "--input"])
        .stdin(Stdio::piped())
        .spawn()
    {
        if let Some(ref mut stdin) = child.stdin {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
    }
}
