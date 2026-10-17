use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crossbeam_channel::Sender;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
use super::picker::{
    PickerItem, PickerKind, PickerPartKind, PickerState, all_tracks_sentinel,
    artist_id_from_sentinel, is_all_tracks_sentinel,
};
use super::picker_items::{load_picker_items, make_album_picker_items};
use super::playlist_modals::{Chooser, NamePrompt, Purpose};
use super::playlists_page::{ListHit, PlAction, PlaylistHit, PlaylistPage, PlaylistsPage};
use super::queue;
use super::search_page::{SearchHit, SearchPage};
use super::settings_page::{Outcome, Setting, SettingsHit, SettingsPage, Task};
use super::theme::{Palette, Theme};
use super::visualizer::VisualizerState;

mod mouse;

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
    /// Choosing which kōan the terminal shows and commands.
    PlayOn,
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
    /// Clear queue, add tracks, play them shuffled.
    Shuffle,
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
    /// `state`, for what reads it off the main thread: the media keys.
    pub shown: Arc<parking_lot::RwLock<Arc<SharedPlayerState>>>,
    /// Where commands go: this player, or the device it controls.
    pub tx: Sender<PlayerCommand>,
    /// Changes of where `tx` goes, in order with what is sent through it.
    lane: super::control::Lane,
    /// This player's own channel, whatever is controlled.
    pub local_tx: Sender<PlayerCommand>,
    pub quit: bool,
    /// When a skip key last acted, to swallow the terminal's key repeat.
    last_skip: Option<std::time::Instant>,

    /// Queue cursor, selection, scroll, and cached snapshot.
    pub queue: QueueState,

    pub picker: Option<PickerState>,
    /// A picker's items, being read on another thread.
    picker_load: Option<(PickerKind, crossbeam_channel::Receiver<Vec<PickerItem>>)>,

    // Spinner tick for download animation.
    pub spinner_tick: usize,

    /// The last click on a row, and when, for telling a double click.
    last_click: Option<(mouse::Click, std::time::Instant)>,

    pub theme: Theme,

    /// Cached layout rects from last render for mouse hit-testing.
    pub layout: LayoutRects,

    /// Threads reading tracks into the queue, each holding the sender of one
    /// of these until it ends. "loading" shows while one runs.
    loading: Vec<crossbeam_channel::Receiver<()>>,

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

    /// The Play on chooser (when in PlayOn mode).
    pub play_on: Option<super::control::PlayOnState>,

    /// What `state` is drawn from: this player, or the device it controls.
    pub view: super::control::View,

    /// EQ chooser state (when in DspSelector mode).
    pub dsp_selector: Option<super::dsp_selector::DspSelectorState>,

    /// Visualizer picker modal state (when in VizPicker mode).
    pub viz_picker: Option<super::viz_picker::VizPickerState>,

    /// Stack of previous modes — push before entering a modal, pop when leaving.
    pub mode_stack: Vec<Mode>,

    /// How far the help is scrolled.
    pub help_scroll: usize,

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
        ticks_per_sec: u8,
    ) -> Self {
        let task_channel = crossbeam_channel::unbounded();
        super::playlist_ops::attach(state.clone(), tx.clone());
        let cfg = koan_core::config::Config::cached();
        let ticker_divisor = (ticks_per_sec / TICKER_FPS).max(1);
        let visualizer = VisualizerState::from_config(&cfg.visualizer);
        let viz_config = cfg.visualizer.clone();
        let (routed, lane) = super::control::router(tx.clone(), task_channel.0.clone());

        Self {
            mode: Mode::Normal,
            state: state.clone(),
            shown: Arc::new(parking_lot::RwLock::new(state.clone())),
            tx: routed,
            lane,
            local_tx: tx,
            quit: false,
            last_skip: None,
            queue: QueueState::default(),
            picker: None,
            picker_load: None,
            spinner_tick: 0,
            last_click: None,
            theme: Theme::new(Palette::detect(&cfg.playback.tui_theme), super::theme::MINT),
            layout: LayoutRects::default(),
            loading: Vec::new(),
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
            play_on: None,
            view: super::control::View::new(state.clone()),
            dsp_selector: None,
            viz_picker: None,
            mode_stack: Vec::new(),
            help_scroll: 0,
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

    /// Ends queue editing, letting the selection go.
    fn leave_queue_edit(&mut self) {
        if self.mode == Mode::QueueEdit {
            self.pop_mode();
        }
        self.queue.selected_ids.clear();
        self.queue.anchor_id = None;
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
        self.say(match (kind, now) {
            (FavouriteKind::Album, true) => "record favourited",
            (FavouriteKind::Album, false) => "record unfavourited",
            (_, true) => "artist favourited",
            (_, false) => "artist unfavourited",
        });
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
        // Against the layout just drawn: what is under the pointer moves with
        // a scroll or a page opened by key, not only with the pointer.
        self.update_hover_zone();
        self.refresh_device_selector();
        self.poll_picker_load();
        self.loading
            .retain(|done| done.try_recv() != Err(crossbeam_channel::TryRecvError::Disconnected));

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

        let playing = self
            .queue
            .vq_cache
            .entries
            .iter()
            .position(|e| e.status == QueueEntryStatus::Playing);
        let playing_path = playing.map(|i| self.queue.vq_cache.entries[i].path.clone());

        // Ticker: advance about `TICKER_FPS` characters a second, restarting
        // when the playing track changes.
        if playing_path != self.ticker_last_path {
            self.ticker_last_path = playing_path.clone();
            self.ticker_offset = 0;
            self.ticker_tick = 0;
        }
        self.ticker_tick = self.ticker_tick.wrapping_add(1);
        if self.ticker_tick.is_multiple_of(self.ticker_divisor) {
            self.ticker_offset = self.ticker_offset.wrapping_add(1);
        }

        // Transient status messages clear after 3 seconds; a copied link stays
        // until it is dismissed.
        if let Some((msg, ts)) = &self.status_message
            && !msg.starts_with("Copied")
            && ts.elapsed().as_secs() >= 3
        {
            self.status_message = None;
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
        // A zoomed cover that has gone would leave a modal showing nothing.
        if self.mode == Mode::CoverArtZoom && self.art.now_playing_art.cached().is_none() {
            self.pop_mode();
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
        while let Ok(report) = self.task_rx.try_recv() {
            let line = match report {
                super::tasks::Report::Task(line) => {
                    self.task_running = false;
                    self.reload_library_pages();
                    line
                }
                super::tasks::Report::Work(line) => {
                    self.reload_library_pages();
                    line
                }
                super::tasks::Report::Say(line) => line,
            };
            self.say(line);
        }
        if super::tasks::take_library_changed() {
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
            if let Some(entry) = playing.map(|i| &self.queue.vq_cache.entries[i]) {
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

        // Fetch the lyrics of a new track while something shows them.
        if (self.lyrics_panel || self.page() == Page::NowPlaying)
            && playing_path.is_some()
            && playing_path != self.lyrics.track_path
            && let Some(entry) = playing.map(|i| &self.queue.vq_cache.entries[i])
        {
            self.lyrics.track_path = playing_path.clone();
            self.lyrics.fetching = true;
            self.lyrics.result = None;
            self.lyrics.lrc_lines.clear();
            let (artist, title, album) = (
                entry.artist.clone(),
                entry.title.clone(),
                entry.album.clone(),
            );
            let duration_secs = entry.duration_ms.unwrap_or(0) / 1000;
            let track_path = entry.path.clone();
            let (tx, rx) = crossbeam_channel::bounded(1);
            self.lyrics_rx = Some(rx);
            std::thread::Builder::new()
                .name("koan-lyrics".into())
                .spawn(move || {
                    let result = (|| {
                        let db = koan_core::db::pool::shared().get().ok()?;
                        let track_id = koan_core::db::queries::track_id_by_path(
                            &db.conn,
                            &track_path.to_string_lossy(),
                        )
                        .ok()
                        .flatten()?;
                        koan_core::lyrics::fetch_lyrics(
                            &db.conn,
                            track_id,
                            &artist,
                            &title,
                            &album,
                            duration_secs,
                        )
                        .inspect_err(|e| {
                            log::warn!("lyrics: fetch failed for '{artist} - {title}': {e}")
                        })
                        .ok()
                    })();
                    let _ = tx.send(result);
                })
                .ok();
        }

        // In normal mode, auto-scroll to playing track on actual track change.
        // Derive the playing track from the visible queue cache (atomic snapshot)
        // NOT from track_info directly — track_info changes before the visible
        // queue is rebuilt, causing a 1-frame scroll offset jump.
        if self.mode == Mode::Normal
            && self.layout.queue_area.height > 0
            && playing_path != self.last_playing_path
        {
            self.last_playing_path = playing_path;
            if let Some(idx) = playing {
                self.queue.scroll_offset = queue::scroll_for_cursor(
                    &self.queue.vq_cache.entries,
                    idx,
                    self.queue.scroll_offset,
                    self.queue_visible_height(),
                );
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
            Mode::PlayOn => self.handle_play_on_key(key),
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

    /// Whether typed text goes into a text field: a prompt, a picker's
    /// query, the genre list's search, or a page's own field.
    pub fn takes_text(&self) -> bool {
        match self.mode {
            Mode::NamePrompt | Mode::Picker(_) => true,
            Mode::FilterPanel => self.filter_panel.as_ref().is_some_and(|p| p.picking()),
            Mode::Normal => self.typing(),
            _ => false,
        }
    }

    /// Types `text` into the field that has the keys, as if each character
    /// were pressed. A paste of several lines becomes one.
    pub fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            let c = if c.is_control() { ' ' } else { c };
            self.handle_key(KeyEvent::from(KeyCode::Char(c)));
        }
    }

    /// Where a thread the TUI started says it has finished.
    pub fn reports(&self) -> crossbeam_channel::Sender<super::tasks::Report> {
        self.task_tx.clone()
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
        // Editing is the queue's; its keys would otherwise follow to a page
        // that has none of what they edit.
        if self.mode == Mode::QueueEdit && self.page() != Page::Queue {
            self.leave_queue_edit();
        }
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
            Page::Album(id) => self.album_page = AlbumPage::load(id),
            Page::Artist(id) => {
                self.artist_page = Browse::new(Source::Artist(id));
                self.artist_page.reload();
            }
            Page::Queue | Page::NowPlaying => {}
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
        // A key a text field let through is still not a command.
        if self.typing() {
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shared = match key.code {
            KeyCode::Char(
                ' ' | '>' | '<' | 'n' | 's' | 'T' | 'R' | '.' | ',' | 'L' | 'V' | 'F' | 'M' | 'X'
                | 'S' | 'v' | 'D' | 'C' | 'E' | 'B' | 'p' | 'q' | '?' | 'z',
            ) => !ctrl,
            // What is playing, put in a playlist; elsewhere the page's own
            // `w` does that with what it shows.
            KeyCode::Char('w' | 'W') => !ctrl && self.page() == Page::NowPlaying,
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
            self.say("this terminal draws no graphics; covers stay in half blocks");
            return;
        }
        let on = self.art.gfx.set_graphics(!self.art.gfx.graphics());
        let _ = koan_core::config::Config::persist(|cfg| cfg.playback.art_graphics = on);
        self.say(if on {
            format!("covers drawn as images ({})", self.art.gfx.protocol_name())
        } else {
            "covers drawn in half blocks".into()
        });
    }

    /// Whether the pages show this device, rather than one it controls.
    /// Its output and EQ are changed only then: they would not be the
    /// controlled device's, and nothing changes it here behind the music.
    fn this_device_shown(&mut self) -> bool {
        if koan_core::remote::devices::target().is_none() {
            return true;
        }
        self.say("output and EQ are this terminal's; choose this terminal in Play on (C) first");
        false
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
            PlAction::Tracks(ids, how) => self.enqueue(ids, how),
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
        let lists = super::playlist_ops::list();
        self.chooser = Some(Chooser::new(ids, &lists));
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
        let Some((target, label)) = chooser.entries.get(idx).cloned() else {
            self.chooser = Some(chooser);
            return;
        };
        self.pop_mode();
        match target {
            None => self.open_name_prompt(Purpose::New(chooser.ids), String::new()),
            Some(id) => {
                let name = label.split(" \u{b7} ").next().unwrap_or("").to_string();
                match super::playlist_ops::add(id, &chooser.ids) {
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

    /// The tracks the queue's cursor or selection stands for.
    fn queue_choice(&self) -> Vec<i64> {
        let visible = self.visible_queue();
        if self.page() == Page::NowPlaying {
            return visible
                .iter()
                .find(|e| e.status == koan_core::player::state::QueueEntryStatus::Playing)
                .and_then(|e| e.db_id)
                .into_iter()
                .collect();
        }
        if !self.queue.selected_ids.is_empty() {
            return visible
                .iter()
                .filter(|e| self.queue.selected_ids.contains(&e.id))
                .filter_map(|e| e.db_id)
                .collect();
        }
        visible
            .get(self.queue.cursor)
            .and_then(|e| e.db_id)
            .into_iter()
            .collect()
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
            Setting::Discoverable | Setting::NearbyControl => {
                koan_core::remote::nearby::reconfigure();
            }
            _ => {}
        }
    }

    fn apply_browse(&mut self, action: browse::Action) {
        match action {
            browse::Action::None => {}
            browse::Action::Play(ids, how) => self.enqueue(ids, how),
            browse::Action::Favourite(id) => {
                self.toggle_favourite(id);
            }
            browse::Action::OpenAlbum(id) => self.go_page(Page::Album(id)),
            browse::Action::OpenArtist(id) => self.go_page(Page::Artist(id)),
            browse::Action::Shuffle(ids) => self.enqueue(ids, PickerAction::Shuffle),
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
                self.say(line);
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
                self.say(if on { "shuffle on" } else { "shuffle off" });
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
                self.say(match next {
                    None => "sleep timer off".into(),
                    Some(SleepTimer::After { minutes }) => format!("sleep in {minutes} min"),
                    Some(SleepTimer::EndOfTrack) => "sleep at the end of the track".into(),
                    Some(SleepTimer::EndOfRecord) => "sleep at the end of the record".into(),
                });
            }
            KeyCode::Char('R') => {
                let repeat = self.state.play_mode().repeat.cycled();
                self.tx.send(PlayerCommand::SetRepeat(repeat)).ok();
                self.say(format!("repeat {}", repeat.as_str()));
            }
            KeyCode::Char('.') | KeyCode::Right => {
                self.seek_to(self.state.position_ms().saturating_add(10_000));
            }
            KeyCode::Char(',') | KeyCode::Left => {
                self.seek_to(self.state.position_ms().saturating_sub(10_000));
            }
            KeyCode::Char('e') => {
                self.push_mode(Mode::QueueEdit);
                // Sync selection to cursor so j/k work immediately.
                if self.queue.selected_ids.len() <= 1 {
                    self.select_single(self.queue.cursor);
                }
            }
            KeyCode::Char('p') => {
                self.open_picker(PickerKind::Track);
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.push_mode(Mode::QueueEdit);
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
            KeyCode::Char('L') => self.lyrics_panel = !self.lyrics_panel,
            KeyCode::Char('V') => {
                self.viz_config.enabled = !self.viz_config.enabled;
                self.say(if self.viz_config.enabled {
                    "visualiser on"
                } else {
                    "visualiser off"
                });
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
                self.say(if self.visualizer.matrix_overlay {
                    "matrix overlay on"
                } else {
                    "matrix overlay off"
                });
                let overlay = self.visualizer.matrix_overlay;
                let _ = koan_core::config::Config::persist(|cfg| {
                    cfg.visualizer.matrix_overlay = overlay;
                });
            }
            KeyCode::Char('S') => {
                self.visualizer.bass_shake = !self.visualizer.bass_shake;
                self.say(if self.visualizer.bass_shake {
                    "bass shake on"
                } else {
                    "bass shake off"
                });
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
                if let Some(e) = visible.get(self.queue.cursor) {
                    match e.db_id {
                        Some(id) => {
                            self.toggle_favourite(id);
                        }
                        None => self.say("that track is not in this library"),
                    }
                }
            }
            KeyCode::Char('z') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.tx.send(PlayerCommand::Undo).ok();
            }
            KeyCode::Char('z') if self.art.now_playing_art.cached().is_some() => {
                self.push_mode(Mode::CoverArtZoom);
            }
            KeyCode::Char('Z')
                if key
                    .modifiers
                    .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT) =>
            {
                self.tx.send(PlayerCommand::Redo).ok();
            }
            KeyCode::Char('D') => {
                if self.this_device_shown() {
                    self.open_device_selector();
                }
            }
            KeyCode::Char('C') => {
                self.play_on = Some(super::control::PlayOnState::new(&self.view.local));
                self.push_mode(Mode::PlayOn);
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
                self.help_scroll = 0;
                self.push_mode(Mode::HelpModal);
            }
            _ => {}
        }
    }

    /// Draw the device being controlled, or this one, and keep the Play on
    /// chooser up with the devices.
    pub fn follow_control(&mut self) {
        let shown = self.view.state().clone();
        if !Arc::ptr_eq(&shown, &self.state) {
            *self.shown.write() = shown.clone();
            self.state = shown;
            // The two states count versions apart.
            self.queue.vq_version = u64::MAX;
        }
        if let Some(p) = self.play_on.as_mut() {
            p.refresh(&self.view.local);
        }
        if self.page() == Page::Settings {
            self.settings_page.follow_devices();
        }
    }

    fn handle_play_on_key(&mut self, key: KeyEvent) {
        let Some(play_on) = self.play_on.as_mut() else {
            self.pop_mode();
            return;
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('C') => {
                self.play_on = None;
                self.pop_mode();
            }
            KeyCode::Up | KeyCode::Char('k') => play_on.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => play_on.move_by(1),
            KeyCode::Enter => {
                let Some(choice) = play_on.current().cloned() else {
                    return;
                };
                if choice.controlled {
                    return;
                }
                let said = format!("controlling {}", choice.name);
                self.run_control(said, move |tx, state| {
                    koan_core::remote::control::control(choice.id, tx, state)
                });
            }
            KeyCode::Char('m') => {
                let Some(choice) = play_on.current().cloned() else {
                    return;
                };
                if !choice.can_move {
                    self.say(format!("nothing to move to {}", choice.name));
                    return;
                }
                let said = format!("moved the music to {}", choice.name);
                self.run_control(said, move |tx, state| {
                    koan_core::remote::control::move_music(choice.id, tx, state, None).map(|_| ())
                });
            }
            KeyCode::Char('x') => {
                let Some(choice) = play_on.current().cloned() else {
                    return;
                };
                let Some(id) = choice.id.filter(|_| choice.away) else {
                    return;
                };
                match koan_core::remote::devices::forget(&id) {
                    Ok(()) => self.say(format!("forgot {}", choice.name)),
                    Err(e) => self.say(e),
                }
            }
            _ => {}
        }
    }

    /// Choosing or moving to a device: on the router's lane, so a key pressed
    /// meanwhile goes where the music went, and off the frame, since a
    /// hand-off waits for this player to go quiet and a wake asks the server.
    fn run_control(
        &mut self,
        said: String,
        work: impl FnOnce(&Sender<PlayerCommand>, &SharedPlayerState) -> Result<(), String>
        + Send
        + 'static,
    ) {
        let (local, tx, report) = (
            self.view.local.clone(),
            self.local_tx.clone(),
            self.task_tx.clone(),
        );
        self.lane.run(move || {
            let line = match work(&tx, &local) {
                Ok(()) => said,
                Err(e) => e,
            };
            report.send(super::tasks::Report::Work(line)).ok();
        });
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

                let current = match self.state.renderer() {
                    Some(output) => Some(renderer_label(&output.name)),
                    None => koan_core::config::Config::cached()
                        .playback
                        .output_device
                        .clone(),
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
    fn refresh_device_selector(&mut self) {
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
            self.pop_mode();
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
        if !self.this_device_shown() {
            return;
        }
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
            self.say("no output device to set an eq on");
            return false;
        };
        let before = profiles::overview_for(Some(device.clone())).active;
        match profiles::assign(name.as_deref(), &device) {
            Ok(()) => {
                if name.is_none() && before.is_some() {
                    self.eq_last = before;
                }
                self.tx.send(PlayerCommand::ReloadDsp).ok();
                self.say(match name {
                    Some(n) => format!("eq: {n}"),
                    None => "eq off".into(),
                });
                true
            }
            Err(e) => {
                self.say(e);
                false
            }
        }
    }

    /// Bypasses the EQ, or puts back what was bypassed. With nothing to put
    /// back, the chooser opens.
    fn toggle_eq(&mut self) {
        if !self.this_device_shown() {
            return;
        }
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
            self.pop_mode();
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
            KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => self.pop_mode(),
            KeyCode::Up | KeyCode::Char('k') => {
                self.help_scroll = self.help_scroll.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => self.help_scroll += 1,
            KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(10),
            KeyCode::PageDown => self.help_scroll += 10,
            KeyCode::Home | KeyCode::Char('g') => self.help_scroll = 0,
            KeyCode::End | KeyCode::Char('G') => self.help_scroll = usize::MAX,
            _ => {}
        }
    }

    fn handle_viz_picker_key(&mut self, key: KeyEvent) {
        let Some(ref mut picker) = self.viz_picker else {
            self.pop_mode();
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
        self.say(format!("visualiser: {}", mode.label()));
        let mode_str = mode.as_str().to_string();
        let _ = koan_core::config::Config::persist(|cfg| {
            cfg.visualizer.mode = mode_str.clone();
        });
    }

    fn handle_edit_key(&mut self, key: KeyEvent) {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Esc => self.leave_queue_edit(),
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
            // Ctrl+D pages, as everywhere else; it never deletes.
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.page_down(false);
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
            KeyCode::Char('K') if self.queue.cursor > 0 => {
                self.queue.cursor -= 1;
                self.extend_selection_to(self.queue.cursor);
                self.update_scroll();
            }
            KeyCode::Char('i') => {
                self.open_track_info(self.queue.cursor);
            }
            KeyCode::Char(' ') if !self.queue.selected_ids.is_empty() => {
                self.open_context_menu();
            }
            // Shift with the paging keys extends the selection. `g` and `G`
            // only move: the terminal reports Shift with `G` itself.
            KeyCode::PageUp => self.page_up(shift),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.page_up(false);
            }
            KeyCode::PageDown => self.page_down(shift),
            KeyCode::Home => self.jump_to_start(shift),
            KeyCode::End => self.jump_to_end(shift),
            KeyCode::Char('g') => self.jump_to_start(false),
            KeyCode::Char('G') => self.jump_to_end(false),
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
            self.pop_mode();
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
        let cfg = koan_core::config::Config::cached();
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
        let cfg = koan_core::config::Config::cached();
        if koan_core::helpers::subsonic_client(&cfg).is_none() {
            self.say(koan_core::helpers::remote_unavailable(&cfg));
            return;
        }
        let Ok(db) = koan_core::db::pool::shared().get() else {
            self.say("the library could not be opened");
            return;
        };
        let track_ids: Vec<i64> = self
            .visible_queue()
            .iter()
            .filter(|e| self.queue.selected_ids.contains(&e.id))
            .filter_map(|e| {
                e.db_id.or_else(|| {
                    koan_core::db::queries::track_id_by_path(&db.conn, &e.path.to_string_lossy())
                        .ok()
                        .flatten()
                })
            })
            .collect();
        let tx = self.task_tx.clone();
        // Off the UI thread: this is a network round trip.
        std::thread::Builder::new()
            .name("koan-share".into())
            .spawn(move || {
                let line = match koan_core::helpers::create_share(
                    &db,
                    koan_core::db::queries::LOCAL_USER,
                    &cfg,
                    &koan_core::helpers::ShareTarget::Tracks(track_ids),
                    None,
                ) {
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
                    Err(e) => format!("Share failed: {e}"),
                };
                tx.send(super::tasks::Report::Say(line)).ok();
            })
            .ok();
        self.say("Creating share...");
    }

    fn open_organize_modal(&mut self) {
        let config = koan_core::config::Config::cached();
        let mut patterns: Vec<(String, String)> =
            config.organize.patterns.clone().into_iter().collect();
        patterns.sort_by(|a, b| a.0.cmp(&b.0));
        let default_pattern = config
            .organize
            .default
            .as_ref()
            .and_then(|name| patterns.iter().position(|(n, _)| n == name))
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
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let action = match key.code {
            KeyCode::Esc => {
                // Closes it whether or not its items have arrived.
                self.close_picker();
                return;
            }
            KeyCode::Enter if ctrl => Some(PickerAction::AppendAndPlay),
            KeyCode::Char('r') if ctrl => Some(PickerAction::ReplaceQueue),
            KeyCode::Enter => Some(PickerAction::Append),
            _ => None,
        };
        if let Some(action) = action {
            self.confirm_picker(action);
            return;
        }
        let Some(picker) = self.picker.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Up => picker.move_up(),
            KeyCode::Down => picker.move_down(),
            KeyCode::PageUp => picker.page_up(10),
            KeyCode::PageDown => picker.page_down(10),
            KeyCode::Home => picker.jump_to_start(),
            KeyCode::End => picker.jump_to_end(),
            KeyCode::Tab => picker.toggle_select(),
            KeyCode::Backspace => picker.backspace(),
            KeyCode::Char('u') if ctrl => picker.page_up(10),
            KeyCode::Char('d') if ctrl => picker.page_down(10),
            KeyCode::Char(c) if !ctrl => picker.type_char(c),
            _ => {}
        }
    }

    fn close_picker(&mut self) {
        self.picker = None;
        self.pop_mode();
    }

    /// Acts on what the picker has chosen, and closes it.
    fn confirm_picker(&mut self, action: PickerAction) {
        let Some(picker) = self.picker.as_ref() else {
            return;
        };
        let (kind, ids) = (picker.kind, picker.confirm());
        self.close_picker();
        let Some(&first) = ids.first() else {
            return;
        };
        match kind {
            PickerKind::Track => self.enqueue(ids, action),
            PickerKind::Album => self.enqueue_albums(ids, action),
            PickerKind::Artist => self.open_artist_albums(first),
            PickerKind::QueueJump => {
                let idx = first as usize;
                if let Some(entry) = self.queue.vq_cache.entries.get(idx) {
                    self.tx.send(PlayerCommand::Play(entry.id)).ok();
                    self.queue.cursor = idx;
                    // The jumped-to track near the top.
                    self.queue.scroll_offset = queue::scroll_cursor_to_top(
                        &self.queue.vq_cache.entries,
                        idx,
                        self.queue_visible_height(),
                    );
                }
            }
        }
    }

    /// The artist picker's second step: the artist's records, with all their
    /// tracks at once at the top. An artist without records plays their
    /// tracks.
    fn open_artist_albums(&mut self, artist_id: i64) {
        let albums = koan_core::db::pool::shared()
            .get()
            .ok()
            .and_then(|db| koan_core::db::queries::albums_for_artist(&db.conn, artist_id).ok())
            .unwrap_or_default();
        if albums.is_empty() {
            self.enqueue_albums(
                vec![all_tracks_sentinel(artist_id)],
                PickerAction::AppendAndPlay,
            );
            return;
        }
        let mut items = vec![PickerItem {
            id: all_tracks_sentinel(artist_id),
            display: "all tracks".to_string(),
            match_text: "all tracks".into(),
            parts: vec![("all tracks".into(), PickerPartKind::Plain)],
        }];
        items.extend(make_album_picker_items(&albums));
        self.picker = Some(PickerState::new(PickerKind::Album, items, false));
        self.push_mode(Mode::Picker(PickerKind::Album));
    }

    /// Puts library tracks in the queue as `how` says, on a thread of their
    /// own: each is read from the library first.
    pub(super) fn enqueue(&mut self, ids: Vec<i64>, how: PickerAction) {
        if !ids.is_empty() {
            self.spawn_enqueue(how, move || ids);
        }
    }

    /// The album picker's records, each in its tracks, or an artist's whole
    /// catalogue for its "all tracks" row.
    fn enqueue_albums(&mut self, ids: Vec<i64>, how: PickerAction) {
        self.spawn_enqueue(how, move || {
            use koan_core::db::queries;
            let Ok(db) = koan_core::db::pool::shared().get() else {
                return Vec::new();
            };
            ids.iter()
                .flat_map(|&id| {
                    if is_all_tracks_sentinel(id) {
                        queries::tracks_for_artist(&db.conn, artist_id_from_sentinel(id))
                    } else {
                        queries::tracks_for_album(&db.conn, id)
                    }
                    .unwrap_or_default()
                })
                .map(|t| t.id)
                .collect()
        });
    }

    fn spawn_enqueue(
        &mut self,
        how: PickerAction,
        tracks: impl FnOnce() -> Vec<i64> + Send + 'static,
    ) {
        let tx = self.tx.clone();
        let playing = self.state.cursor();
        let (running, done) = crossbeam_channel::bounded::<()>(0);
        let spawned = std::thread::Builder::new()
            .name("koan-enqueue".into())
            .spawn(move || {
                let _running = running;
                let ids = tracks();
                if !ids.is_empty() {
                    super::enqueue::enqueue_playlist(ids, how, playing, tx);
                }
            });
        if spawned.is_ok() {
            self.loading.push(done);
        }
    }

    /// Whether tracks are still being read into the queue.
    pub fn loading(&self) -> bool {
        !self.loading.is_empty()
    }

    /// Shows "loading" until `done` disconnects: its sender is held by a
    /// thread reading tracks into the queue.
    pub fn wait_for(&mut self, done: crossbeam_channel::Receiver<()>) {
        self.loading.push(done);
    }

    fn handle_info_key(&mut self, key: KeyEvent) {
        if matches!(
            key.code,
            KeyCode::Esc | KeyCode::Char('i') | KeyCode::Char('q')
        ) {
            self.close_info();
        }
    }

    fn close_info(&mut self) {
        self.track_details = None;
        self.art.cover_art.clear();
        self.pop_mode();
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
            self.push_mode(Mode::TrackDetails);
        }
    }

    fn handle_zoom_key(&mut self, key: KeyEvent) {
        if matches!(
            key.code,
            KeyCode::Esc | KeyCode::Char('z') | KeyCode::Char('q')
        ) {
            self.pop_mode();
        }
    }

    fn open_track_info(&mut self, idx: usize) {
        let Some(entry) = self.queue.vq_cache.entries.get(idx) else {
            return;
        };
        let (id, path) = (entry.id, entry.path.clone());
        self.push_mode(Mode::TrackInfo(id));
        // Prime the cache for this track's path.
        self.art.cover_art.get(&path);
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
                self.say("that track is not in the library");
            }
        }
    }

    /// Whether the artists tree is the page in front.
    pub fn library_active(&self) -> bool {
        self.mode == Mode::Normal && self.page() == Page::Artists && self.library.is_some()
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
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
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
                    self.enqueue(ids, PickerAction::AppendAndPlay);
                }
            }
            KeyCode::Left | KeyCode::Backspace | KeyCode::Char('h') => {
                lib.collapse_or_parent();
            }
            KeyCode::Char('a') => {
                let ids = lib.enqueue_all_under_cursor();
                self.enqueue(ids, PickerAction::Append);
            }
            KeyCode::Char('w') => {
                let ids = lib.enqueue_all_under_cursor();
                self.open_chooser(ids);
            }
            KeyCode::Char('N') => {
                let ids = lib.enqueue_all_under_cursor();
                self.enqueue(ids, PickerAction::PlayNext);
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

    /// Opens a picker of `kind`; its items are read on another thread and it
    /// fills in when they arrive.
    fn open_picker(&mut self, kind: PickerKind) {
        self.push_mode(Mode::Picker(kind));
        if self.picker_load.as_ref().is_some_and(|(k, _)| *k == kind) {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        let spawned = std::thread::Builder::new()
            .name("koan-picker".into())
            .spawn(move || {
                let _ = tx.send(load_picker_items(kind));
            });
        self.picker_load = spawned.is_ok().then_some((kind, rx));
    }

    /// Fills the picker in with the items read for it, if it is still open.
    fn poll_picker_load(&mut self) {
        let Some((kind, rx)) = self.picker_load.as_ref() else {
            return;
        };
        let kind = *kind;
        match rx.try_recv() {
            Ok(items) => {
                self.picker_load = None;
                if self.mode == Mode::Picker(kind) && self.picker.is_none() {
                    let multi = kind == PickerKind::Track;
                    self.picker = Some(PickerState::new(kind, items, multi));
                }
            }
            Err(crossbeam_channel::TryRecvError::Empty) => {}
            Err(crossbeam_channel::TryRecvError::Disconnected) => self.picker_load = None,
        }
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
        self.push_mode(Mode::Picker(PickerKind::QueueJump));
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

    fn update_scroll(&mut self) {
        let visible = self.visible_queue();
        let visible_height = self.queue_visible_height();
        self.queue.scroll_offset = queue::scroll_for_cursor(
            visible,
            self.queue.cursor,
            self.queue.scroll_offset,
            visible_height,
        );
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
            // A queue that shrank under the scroll would draw its last line
            // alone at the top, and clicks would land on rows not drawn.
            let max = queue::display_line_count(&self.queue.vq_cache.entries)
                .saturating_sub(self.queue_visible_height());
            self.queue.scroll_offset = self.queue.scroll_offset.min(max);
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
        self.say(format!("can't play — {fresh}"));
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
            self.say(format!(
                "still downloading{so_far} — seekable once it lands"
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
            self.close_info();
        }
    }

    pub fn visible_queue(&self) -> &[QueueEntry] {
        &self.queue.vq_cache.entries
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
