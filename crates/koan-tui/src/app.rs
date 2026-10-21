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

use super::accent::Accent;
use super::actions_menu::ActionsMenu;
use super::album_page::{AlbumHit, AlbumPage};
use super::browse::{self, Browse, Source};
use super::browse_filter::FilterPanel;
use super::history_page::{HistoryHit, HistoryPage};
use super::lyrics::LyricsState;
use super::nav::{Navigator, Page};
use super::picker::{PickerKind, PickerPartKind, PickerState};
use super::playlist_modals::{Chooser, NamePrompt, Purpose};
use super::playlists_page::{ListHit, PlAction, PlaylistHit, PlaylistPage, PlaylistsPage};
use super::queue;
use super::settings_page::{Outcome, Setting, SettingsHit, SettingsPage, Task};
use super::shelf_page::{ShelfHit, ShelfKind, ShelfPage};
use super::theme::{Palette, Theme};
use super::transport::Control;
use super::visualizer::VisualizerState;

mod mouse;

/// Characters per second the transport ticker scrolls when the title overflows.
const TICKER_FPS: u8 = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
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
    /// Asking before a row replaces a queue that has something in it.
    ReplaceQueue,
    HelpModal,
    VizPicker,
}

/// Whether a queue entry's file is still on its way, so a cover read from it
/// now may be missing.
fn arriving(entry: &QueueEntry) -> bool {
    entry.download_progress.is_some()
        || matches!(
            entry.status,
            QueueEntryStatus::Downloading | QueueEntryStatus::PriorityPending
        )
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
    /// The artist an album heading is filed under.
    GoToAlbumArtist,
    AddToPlaylist,
    Download,
    RemoveDownload,
    /// A rule between groups of items.
    Separator,
}

/// A status bar message with the instant it was created (for auto-clear timeout).
type StatusMessage = (String, std::time::Instant);

pub struct ContextMenuState {
    /// (action, label, hotkey char)
    pub actions: Vec<(ContextAction, &'static str, char)>,
    pub cursor: usize,
    /// Where it was opened by a click; one opened by a key is centred.
    pub at: Option<(u16, u16)>,
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
    /// Clear queue, add tracks, play from the one at this index: a list
    /// played from a track in it.
    ReplaceFrom(usize),
    /// Clear queue, add tracks, play them shuffled.
    Shuffle,
}

/// A drag of the selected tracks through the queue.
pub struct DragState {
    /// How many selected tracks sit above the one grabbed, which stays under
    /// the pointer.
    pub anchor_offset: usize,
    /// Where the selection was last asked to start, so a move is sent only
    /// when the pointer crosses a row.
    pub group_start: usize,
}

/// Which UI element the mouse cursor is currently hovering over.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub enum HoverZone {
    #[default]
    None,
    QueueItem(usize),
    Browse(browse::Hit),
    Album(AlbumHit),
    History(HistoryHit),
    Shelf(ShelfHit),
    Settings(SettingsHit),
    Playlists(ListHit),
    Playlist(PlaylistHit),
    Downloads(super::downloads_page::DownloadsHit),
    Nav(NavTarget),
}

/// What a click in the sidebar or the tab bar goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavTarget {
    Page(Page),
    Back,
    Forward,
    /// The sidebar's "new playlist" row.
    NewPlaylist,
    /// The stop button on the running scan's row.
    CancelTask,
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
    /// The queue's lines, built when it changes or is regrouped.
    pub(super) layout: queue::Layout,
    /// What the library says of the queue's tracks, read when it changes.
    pub(super) facts: queue::Facts,
    /// A row per track rather than album runs.
    pub flat: bool,
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
    pub now_playing_art_area: ratatui::layout::Rect,
    pub transport_text_area: ratatui::layout::Rect,
    /// The transport's controls and seek bar, and Now playing's.
    pub transport: super::transport::Hits,
    pub now_playing: super::transport::Hits,
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
    /// Back, forward and the page's controls in the toolbar row.
    pub toolbar_hits: Vec<(ratatui::layout::Rect, super::shell::ToolHit)>,
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

    pub nav: Navigator,
    pub albums_page: Browse,
    pub artists_page: Browse,
    pub tracks_page: Browse,
    pub favourites_page: ShelfPage,
    pub recent_page: ShelfPage,
    pub downloaded_page: ShelfPage,
    pub artist_page: Browse,
    pub album_page: Option<AlbumPage>,
    /// The scan, sync or forgetting running, one at a time.
    pub activity: Option<Arc<super::tasks::Activity>>,
    task_tx: crossbeam_channel::Sender<super::tasks::Report>,
    task_rx: crossbeam_channel::Receiver<super::tasks::Report>,
    pub track_details: Option<Box<super::track_info::Details>>,
    /// The EQ page, and what the output plays for Now Playing's curve.
    pub eq_page: super::eq_page::EqPage,
    pub history_page: HistoryPage,
    pub search_page: ShelfPage,
    pub playlists_page: PlaylistsPage,
    pub playlist_page: Option<PlaylistPage>,
    pub downloads_page: super::downloads_page::DownloadsPage,
    pub name_prompt: Option<NamePrompt>,
    pub chooser: Option<Chooser>,
    pub actions_menu: Option<ActionsMenu>,
    pub filter_panel: Option<FilterPanel>,
    pub replace_dialog: Option<super::replace_dialog::ReplaceDialog>,
    /// The filters on the album, artist and track browsers, shared by them
    /// as the apps' are.
    pub browse_filters: koan_core::browse::BrowseFilter,
    pub settings_page: SettingsPage,
    /// A drag that began on a page's scrollbar.
    browse_drag: bool,
    pub thumbs: browse::Thumbs,
    /// The playing record's accent, easing as the record changes.
    pub accent: Accent,
    pub palette: Palette,
    theme_accent: (u8, u8, u8),
    /// The config as of this tick, read once for the tick and its frame.
    pub cfg: Arc<koan_core::config::Config>,
    /// The playing track's queue entry, as the cover needs it: its id, the
    /// queue version it was found at, whether its file is still arriving,
    /// and its row. Looked up again only when one of those can have moved.
    playing_entry: Option<(QueueItemId, u64, bool, Option<i64>)>,
    /// Whether the file of the track info popup's entry is still arriving,
    /// so its cover is asked for again until it has.
    info_arriving: bool,
    /// What Now playing keeps between frames.
    pub now_playing: super::now_playing::State,

    pub art: ArtState,

    /// Context menu state (when in ContextMenu mode).
    pub context_menu: Option<ContextMenuState>,

    /// Organize modal state (when in Organize mode).
    pub organize: Option<super::organize::OrganizeModalState>,

    /// Device selector modal state (when in DeviceSelector mode).
    pub device_selector: Option<super::device_selector::OutputPicker>,

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

    /// The artist and album links drawn this frame, wherever they are.
    pub links: super::shell::Links,

    /// Config writes, applied in the order they were made, off the frame.
    config_tx: Sender<ConfigChange>,

    /// The playlists the sidebar lists, by id and name.
    pub sidebar_playlists: Vec<(i64, String)>,

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
        // EQ belongs to this device's player, whatever the TUI controls.
        let eq_page = super::eq_page::EqPage::new(tx.clone());

        Self {
            mode: Mode::Normal,
            state: state.clone(),
            shown: Arc::new(parking_lot::RwLock::new(state.clone())),
            tx: routed,
            lane,
            local_tx: tx,
            quit: false,
            last_skip: None,
            queue: QueueState {
                flat: !cfg.playback.queue_grouped,
                ..Default::default()
            },
            picker: None,
            spinner_tick: 0,
            last_click: None,
            theme: Theme::new(Palette::detect(&cfg.playback.tui_theme), super::theme::MINT),
            layout: LayoutRects::default(),
            loading: Vec::new(),
            last_playing_path: None,
            nav: Navigator::default(),
            albums_page: Browse::new(Source::Albums),
            artists_page: Browse::new(Source::Artists),
            tracks_page: Browse::new(Source::Tracks),
            favourites_page: ShelfPage::new(ShelfKind::Favourites),
            recent_page: ShelfPage::new(ShelfKind::Recent),
            downloaded_page: ShelfPage::new(ShelfKind::Downloaded),
            artist_page: Browse::new(Source::Artist(0)),
            album_page: None,
            activity: None,
            task_tx: task_channel.0,
            task_rx: task_channel.1,
            track_details: None,
            eq_page,
            history_page: HistoryPage::new(),
            search_page: ShelfPage::new(ShelfKind::Search),
            playlists_page: PlaylistsPage::new(),
            playlist_page: None,
            downloads_page: Default::default(),
            name_prompt: None,
            chooser: None,
            actions_menu: None,
            filter_panel: None,
            replace_dialog: None,
            browse_filters: Default::default(),
            settings_page: SettingsPage::new(),
            browse_drag: false,
            thumbs: browse::Thumbs::new(),
            accent: Accent::default(),
            palette: Palette::detect(&cfg.playback.tui_theme),
            now_playing: Default::default(),
            theme_accent: super::theme::MINT,
            cfg: koan_core::config::Config::cached(),
            playing_entry: None,
            info_arriving: false,
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
            links: Default::default(),
            sidebar_playlists: Vec::new(),
            config_tx: config_writer(),
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

    /// Writes `change` to the config on the writer thread, after every change
    /// asked for before it.
    pub(super) fn persist(
        &self,
        change: impl FnOnce(&mut koan_core::config::Config) + Send + 'static,
    ) {
        self.config_tx.send(Box::new(change)).ok();
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
        self.say(match (kind, now) {
            (FavouriteKind::Album, true) => "album favourited",
            (FavouriteKind::Album, false) => "album unfavourited",
            (_, true) => "artist favourited",
            (_, false) => "artist unfavourited",
        });
        for page in [
            &mut self.albums_page,
            &mut self.artists_page,
            &mut self.artist_page,
        ] {
            page.reload();
        }
        // A shelf of favourites loses what was just let go; the others
        // show the hearts.
        self.reload_shelves();
        if let Some(p) = self.album_page.as_mut()
            && kind == FavouriteKind::Album
            && p.album.id == id
        {
            p.favourite = now;
        }
    }

    pub fn handle_tick(&mut self) {
        self.cfg = koan_core::config::Config::cached();
        // Refresh visible queue cache so all tick logic sees current state.
        self.refresh_visible_queue();
        // Against the layout just drawn: what is under the pointer moves with
        // a scroll or a page opened by key, not only with the pointer.
        self.update_hover_zone();
        self.refresh_device_selector();
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

        // The playing track's cover, and the accent it gives once it arrives.
        let now = std::time::Instant::now();
        match self.state.track_info() {
            Some(info) => {
                let version = self.queue.vq_version;
                // A file still arriving moves without the version moving.
                let stale = self.playing_entry.is_none_or(|(id, seen, arriving, _)| {
                    id != info.id || seen != version || arriving
                });
                if stale {
                    let entry = self.queue.vq_cache.entries.iter().find(|e| e.id == info.id);
                    self.playing_entry = Some((
                        info.id,
                        version,
                        entry.is_some_and(arriving),
                        entry.and_then(|e| e.db_id),
                    ));
                }
                if let Some((_, _, arriving, track)) = self.playing_entry {
                    self.art
                        .now_playing_art
                        .want(&info.path, arriving, || track);
                }
            }
            None => {
                self.playing_entry = None;
                self.art.now_playing_art.clear();
            }
        }
        // A zoomed cover that has gone would leave a modal showing nothing.
        if self.mode == Mode::CoverArtZoom && self.art.now_playing_art.cached().is_none() {
            self.pop_mode();
        }
        let np = &mut self.art.now_playing_art;
        np.poll();
        // Taken every tick, so turning record colours on or off lands at once.
        let colours = self.cfg.appearance.record_colours;
        let rgb = np.cover().and_then(|c| c.accent).filter(|_| colours);
        self.accent.set(rgb.unwrap_or(super::theme::MINT), now);
        if let Mode::TrackInfo(id) = self.mode
            && self.info_arriving
            && let Some(entry) = self.queue.vq_cache.entries.iter().find(|e| e.id == id)
        {
            self.info_arriving = arriving(entry);
            let (path, track) = (entry.path.clone(), entry.db_id);
            self.art.cover_art.want(&path, self.info_arriving, || track);
        }
        self.art.cover_art.poll();
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
                    self.activity = None;
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
        if self.page() == Page::History
            && let Some(error) = self.history_page.poll()
        {
            self.say(error);
        }
        if self.page() == Page::Settings
            && let Some(said) = self.settings_page.poll()
        {
            self.say(said);
        }
        for page in [
            &mut self.albums_page,
            &mut self.artists_page,
            &mut self.tracks_page,
            &mut self.artist_page,
        ] {
            page.poll();
        }
        for page in [
            &mut self.favourites_page,
            &mut self.recent_page,
            &mut self.downloaded_page,
            &mut self.search_page,
        ] {
            page.poll();
        }
        for outcome in self.eq_page.poll() {
            // The EQ page says it on itself.
            if let super::eq_page::Outcome::Say(said) = outcome
                && self.page() != Page::Eq
            {
                self.say(said);
            }
        }
        if self.page() == Page::NowPlaying {
            let renderer = self.state.renderer().map(|r| r.udn);
            let rate = self
                .state
                .output_sample_rate()
                .or_else(|| self.state.track_info().map(|t| t.sample_rate))
                .unwrap_or(super::eq_worker::RATE);
            let key = (self.state.dsp().map(|d| d.profile), renderer.clone(), rate);
            if self.eq_page.playing_key.as_ref() != Some(&key) {
                self.eq_page.playing_key = Some(key);
                self.eq_page.request_playing(renderer, rate);
            }
        }

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
                self.visualizer.now_artist = self.queue.facts.credits_of(entry).artist;
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
                    &self.queue.layout,
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
            Mode::ReplaceQueue => {
                if let Some(choice) = self.replace_dialog.as_mut().and_then(|d| d.handle_key(key)) {
                    self.answer_replace(choice);
                }
            }
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
            Page::Albums | Page::Artists | Page::Tracks | Page::Artist(_) => {
                self.browse().is_some_and(|b| b.filter_active)
            }
            Page::History => self.history_page.filter_active,
            Page::Search => self.search_page.typing,
            Page::Playlists => self.playlists_page.filter_active,
            Page::Queue
            | Page::NowPlaying
            | Page::Favourites
            | Page::Recent
            | Page::Downloaded
            | Page::Album(_)
            | Page::Playlist(_)
            | Page::Downloads => false,
            Page::Settings => self.settings_page.typing(),
            Page::Eq => self.eq_page.takes_keys(),
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
            Page::Artists => Some(&self.artists_page),
            Page::Tracks => Some(&self.tracks_page),
            Page::Artist(_) => Some(&self.artist_page),
            _ => None,
        }
    }

    pub fn browse_mut(&mut self) -> Option<&mut Browse> {
        match self.page() {
            Page::Albums => Some(&mut self.albums_page),
            Page::Artists => Some(&mut self.artists_page),
            Page::Tracks => Some(&mut self.tracks_page),
            Page::Artist(_) => Some(&mut self.artist_page),
            _ => None,
        }
    }

    /// The shelf or search on screen.
    pub fn shelf_mut(&mut self) -> Option<&mut ShelfPage> {
        match self.page() {
            Page::Favourites => Some(&mut self.favourites_page),
            Page::Recent => Some(&mut self.recent_page),
            Page::Downloaded => Some(&mut self.downloaded_page),
            Page::Search => Some(&mut self.search_page),
            _ => None,
        }
    }

    pub fn shelf(&self) -> Option<&ShelfPage> {
        match self.page() {
            Page::Favourites => Some(&self.favourites_page),
            Page::Recent => Some(&self.recent_page),
            Page::Downloaded => Some(&self.downloaded_page),
            Page::Search => Some(&self.search_page),
            _ => None,
        }
    }

    /// Has the shelves read themselves again: what is on them changed.
    fn reload_shelves(&mut self) {
        for page in [
            &mut self.favourites_page,
            &mut self.recent_page,
            &mut self.downloaded_page,
            &mut self.search_page,
        ] {
            page.reload();
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
            Page::Artists => self.artists_page.reload(),
            Page::Tracks => self.tracks_page.reload(),
            Page::Favourites | Page::Recent | Page::Downloaded | Page::Search => {
                if let Some(page) = self.shelf_mut() {
                    page.open();
                }
            }
            Page::History => self.history_page.reload(),
            Page::Playlists => self.playlists_page.reload(),
            Page::Playlist(id) => self.playlist_page = PlaylistPage::load(id),
            Page::Settings => {
                self.settings_page.reload();
                self.settings_page.set_eq_summary(self.eq_page.summary());
            }
            Page::Eq => self
                .eq_page
                .open(self.state.renderer().map(|r| (r.udn, r.name))),
            Page::Album(id) => self.album_page = AlbumPage::load(id),
            Page::Artist(id) => {
                self.artist_page = Browse::new(Source::Artist(id));
                self.artist_page.reload();
            }
            Page::Queue | Page::NowPlaying | Page::Downloads => {}
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
            NavTarget::NewPlaylist => {
                self.open_name_prompt(Purpose::New(Vec::new()), String::new())
            }
            NavTarget::CancelTask => self.cancel_task(),
        }
    }

    /// What the page in front shows in the toolbar row. Each page's own
    /// controls are its arm here.
    pub fn toolbar(&self) -> super::shell::Toolbar {
        use super::shell::Toolbar;
        match self.page() {
            Page::Queue => queue::toolbar(
                &self.queue.vq_cache.entries,
                &self.queue.layout,
                &self.queue.facts,
                self.queue.selected_ids.len(),
            ),
            Page::Playlists => self.playlists_page.toolbar(),
            Page::Playlist(_) => match self.playlist_page.as_ref() {
                Some(p) => p.toolbar(),
                None => Toolbar::new("playlist"),
            },
            Page::Downloads => self.downloads_page.toolbar(),
            Page::History => self.history_page.toolbar(),
            Page::Albums | Page::Artists | Page::Tracks | Page::Artist(_) => self
                .browse()
                .map(Browse::toolbar)
                .unwrap_or_else(|| Toolbar::new(self.page().title())),
            Page::Favourites | Page::Recent | Page::Downloaded | Page::Search => self
                .shelf()
                .map(ShelfPage::toolbar)
                .unwrap_or_else(|| Toolbar::new(self.page().title())),
            Page::Album(_) => match self.album_page.as_ref() {
                Some(p) => p.toolbar(),
                None => Toolbar::new("album"),
            },
            Page::NowPlaying => {
                use super::shell::ToolItem;
                let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
                Toolbar::new("now playing")
                    .item(ToolItem::new("cover", key('z')).priority(1))
                    .item(ToolItem::new("visualiser", key('M')).priority(2))
                    .item(ToolItem::new("play on", key('C')).priority(3))
                    .item(
                        ToolItem::new("eq", key('E'))
                            .on(self.state.dsp().is_some())
                            .priority(4),
                    )
            }
            Page::Settings => self.settings_page.toolbar(),
            Page::Eq => self.eq_page.toolbar(),
        }
    }

    /// A click on the toolbar or on a link. True when it was one.
    fn shell_click(&mut self, col: u16, row: u16) -> bool {
        use super::shell::{Link, ToolHit};
        let toolbar = self
            .layout
            .toolbar_hits
            .iter()
            .find(|(r, _)| r.contains(ratatui::layout::Position::new(col, row)))
            .map(|(_, h)| *h);
        match toolbar {
            Some(ToolHit::Nav(target)) => self.activate(target),
            Some(ToolHit::Key(key)) => self.handle_key(key),
            None => match self.links.at(col, row) {
                Some(Link::Artist(id)) => self.go_page(Page::Artist(id)),
                Some(Link::Album(id)) => self.go_page(Page::Album(id)),
                Some(Link::ArtistOfTrack(id)) => self.open_entry_link(Some(id), false),
                Some(Link::AlbumOfTrack(id)) => self.open_entry_link(Some(id), true),
                Some(Link::AlbumArtistOfTrack(id)) => self.open_credited_artist(Some(id), true),
                None => return false,
            },
        }
        true
    }

    /// Reads the playlists the sidebar lists.
    pub fn refresh_sidebar_playlists(&mut self) {
        self.sidebar_playlists = super::playlist_ops::list()
            .into_iter()
            .map(|p| (p.id, p.name))
            .collect();
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
            Page::Downloads => match self.downloads_page.handle_key(key, &self.state) {
                Some(action) => self.apply_browse(action),
                None if key.code == KeyCode::Esc => self.leave_page(),
                None => self.handle_transport_key(key),
            },
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
            Page::Search | Page::Favourites | Page::Recent | Page::Downloaded => {
                let action = self.shelf_mut().and_then(|p| p.handle_key(key));
                match action {
                    Some(action) => self.apply_browse(action),
                    None if key.code == KeyCode::Esc => self.leave_page(),
                    None if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) => {
                        self.navigate_key(&key);
                    }
                    None => self.handle_transport_key(key),
                }
            }
            Page::Eq => match self.eq_page.handle_key(key) {
                Some(Some(super::eq_page::Outcome::Leave)) => self.leave_page(),
                Some(Some(super::eq_page::Outcome::Say(said))) => self.say(said),
                Some(None) => {}
                None if key.code == KeyCode::Esc => self.leave_page(),
                None => self.handle_transport_key(key),
            },
            Page::History => match self.history_page.handle_key(key, &self.favourites) {
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
            Page::Albums | Page::Artists | Page::Tracks | Page::Artist(_) => {
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
                | 'S' | 'D' | 'C' | 'E' | 'B' | 'p' | 'q' | '?' | 'z',
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
        self.persist(move |cfg| cfg.playback.art_graphics = on);
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
            PlAction::ArtistOfTrack(id) => {
                self.open_credited(koan_core::db::queries::CreditsOf::Tracks, id)
            }
            PlAction::Info(id) => self.open_track_details(id),
            PlAction::New => self.open_name_prompt(Purpose::New(Vec::new()), String::new()),
            PlAction::Rename(id, name) => self.open_name_prompt(Purpose::Rename(id), name),
            PlAction::Choose(ids) => self.open_chooser(ids),
            PlAction::Say(text) => self.say(text),
            PlAction::Favourite(id) => {
                self.toggle_favourite(id);
            }
            PlAction::Menu(title, items) => self.open_actions_menu(title, items),
            PlAction::Export(id, name) => {
                let file = name.replace('/', "-");
                self.open_name_prompt(Purpose::Export(id), format!("~/{file}.m3u8"));
            }
            PlAction::Deleted(name) => {
                self.say(format!("deleted '{name}'"));
                self.playlists_changed();
                self.go_page(Page::Playlists);
            }
            PlAction::Page(act) => {
                let next = self
                    .playlist_page
                    .as_mut()
                    .map_or(PlAction::None, |p| p.apply(act));
                self.apply_playlist(next);
            }
            PlAction::Play(id, shuffled) => {
                let entries = super::playlist_ops::entries(id);
                match super::playlist_ops::queue_command(&entries, None, shuffled) {
                    Some(cmd) => {
                        self.tx.send(cmd).ok();
                    }
                    None => self.say("nothing in it to play"),
                }
            }
            PlAction::Delete(id) => {
                let name = super::playlist_ops::get(id)
                    .map(|p| p.name)
                    .unwrap_or_default();
                match super::playlist_ops::delete(id) {
                    Ok(()) => self.say(format!("deleted '{name}'")),
                    Err(e) => self.say(e),
                }
                self.playlists_changed();
            }
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
            Purpose::Export(id) => match super::playlist_ops::export(*id, &prompt.text) {
                Ok(said) => said,
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
        self.say(said);
    }

    /// Has the pages that show playlists read them again.
    fn playlists_changed(&mut self) {
        self.refresh_sidebar_playlists();
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
        let filters = panel.filters.clone();
        self.set_browse_filters(filters);
    }

    fn set_browse_filters(&mut self, filters: koan_core::browse::BrowseFilter) {
        for page in [
            &mut self.albums_page,
            &mut self.artists_page,
            &mut self.tracks_page,
        ] {
            page.set_filters(&filters);
        }
        self.browse_filters = filters;
        if let Some(browse) = self.browse_mut() {
            browse.ensure_loaded();
        }
    }

    /// Does what the replace dialog was answered with, and closes it.
    pub(super) fn answer_replace(&mut self, choice: super::replace_dialog::Choice) {
        use super::replace_dialog::Choice;
        let dialog = self.replace_dialog.take();
        self.pop_mode();
        let Some(d) = dialog else {
            return;
        };
        match choice {
            Choice::Play => self.enqueue(d.ids, PickerAction::ReplaceFrom(d.start)),
            Choice::Add => self.enqueue(d.ids, PickerAction::Append),
            Choice::Cancel => {}
        }
    }

    fn handle_filter_panel_key(&mut self, key: KeyEvent) {
        let Some(panel) = self.filter_panel.as_mut() else {
            self.pop_mode();
            return;
        };
        match panel.handle_key(key) {
            None => self.close_filter_panel(),
            Some(true) => self.filters_changed(),
            Some(false) => {}
        }
    }

    fn close_actions_menu(&mut self) {
        self.actions_menu = None;
        self.pop_mode();
    }

    /// Runs the menu's item at `idx` as its key would, and closes the menu.
    fn run_menu_item(&mut self, idx: usize) {
        if self
            .actions_menu
            .as_ref()
            .and_then(|m| m.items.get(idx))
            .is_some_and(|(label, _)| label.is_empty())
        {
            return;
        }
        let action = self
            .actions_menu
            .take()
            .and_then(|m| m.items.into_iter().nth(idx))
            .map(|(_, item)| item);
        self.pop_mode();
        match action {
            Some(super::actions_menu::Item::Browse(action)) => self.apply_browse(action),
            Some(super::actions_menu::Item::Playlist(action)) => self.apply_playlist(action),
            None => {}
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

    /// The tracks the queue's selection stands for, or without one the track
    /// under the cursor. Now playing always means the one playing.
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
        let one = if on_queue {
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
            Outcome::Say(said) => self.say(said),
            Outcome::OpenEq => self.open_eq_page(),
            Outcome::OpenOutputs => {
                if self.this_device_shown() {
                    self.open_device_selector();
                }
            }
            Outcome::OpenUrl(url) => self.apply_browse(browse::Action::OpenUrl(url)),
            Outcome::Copy(text) => {
                copy_to_clipboard(&text);
                self.say("copied");
            }
            Outcome::SignedOut { forget } => {
                self.local_tx.send(PlayerCommand::SignedOut).ok();
                if forget {
                    self.run_task(Task::ForgetRemote);
                } else {
                    self.say("signed out");
                }
            }
        }
    }

    /// Starts long work on the library on a thread of its own and says how
    /// it went. Forgetting runs beside a scan: the core's lane orders them.
    fn run_task(&mut self, task: Task) {
        use super::tasks;
        match task {
            Task::Scan { force } => {
                self.spawn_task("scanning the library", true, move |a| tasks::scan(force, a))
            }
            Task::Sync => self.spawn_task("syncing with the server", false, tasks::sync),
            Task::RebuildIndex => self.spawn_task("clearing the library index", false, |_| {
                tasks::rebuild_index()
            }),
            Task::ClearCache => {
                let state = self.view.local.clone();
                self.spawn_task("clearing downloads", false, move |_| {
                    tasks::clear_cache(&state)
                });
            }
            Task::ForgetFolder(path) => self
                .spawn_work("forgetting the folder's tracks...", move || {
                    tasks::forget(&path)
                }),
            Task::ForgetRemote => {
                self.spawn_work("forgetting the server's tracks...", tasks::forget_remote)
            }
        }
    }

    /// Runs `work` beside whatever task is running, and says how it went.
    fn spawn_work(&mut self, said: &str, work: impl FnOnce() -> String + Send + 'static) {
        self.say(said);
        let tx = self.task_tx.clone();
        std::thread::Builder::new()
            .name("koan-work".into())
            .spawn(move || {
                tx.send(super::tasks::Report::Work(work())).ok();
            })
            .ok();
    }

    /// One at a time; the sidebar shows how it is going.
    fn spawn_task(
        &mut self,
        label: &str,
        cancellable: bool,
        work: impl FnOnce(&super::tasks::Activity) -> String + Send + 'static,
    ) {
        if self.activity.is_some() {
            self.say("a scan or sync is already running");
            return;
        }
        let activity = super::tasks::Activity::new(label, cancellable);
        let (tx, running) = (self.task_tx.clone(), activity.clone());
        let spawned = std::thread::Builder::new()
            .name("koan-task".into())
            .spawn(move || {
                tx.send(super::tasks::Report::Task(work(&running))).ok();
            });
        if spawned.is_ok() {
            self.activity = Some(activity);
            self.say(format!("{label}..."));
        }
    }

    /// Stops the running scan, keeping what it has read.
    fn cancel_task(&mut self) {
        if let Some(cancel) = self.activity.as_ref().and_then(|a| a.cancel.as_ref()) {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            self.say("stopping...");
        }
    }

    /// Has the page on screen read the library again: after a scan or a
    /// sync, downloads removed, or offline turned on or off. The other pages
    /// read it as they open.
    fn reload_library_pages(&mut self) {
        self.thumbs.retry_missing();
        match self.page() {
            Page::History => self.history_page.reload(),
            Page::Settings => self.settings_page.reload(),
            // A sync brings profiles edited on the account's other devices.
            Page::Eq => self
                .eq_page
                .follow(self.state.renderer().map(|r| (r.udn, r.name))),
            Page::Album(id) => self.album_page = AlbumPage::load(id),
            Page::Favourites | Page::Recent | Page::Downloaded | Page::Search => {
                if let Some(page) = self.shelf_mut() {
                    page.reload();
                }
            }
            Page::Playlists | Page::Playlist(_) => self.playlists_changed(),
            _ => {
                if let Some(page) = self.browse_mut() {
                    page.reload();
                }
            }
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

    pub(crate) fn apply_browse(&mut self, action: browse::Action) {
        match action {
            browse::Action::None => {}
            // A list played from a row takes the queue's place: asked first
            // when there is something in it, as the apps ask.
            browse::Action::Play(ids, PickerAction::ReplaceFrom(start))
                if !self.visible_queue().is_empty() =>
            {
                let queued = self.visible_queue().len();
                self.replace_dialog = Some(super::replace_dialog::ReplaceDialog::new(
                    ids, start, queued,
                ));
                self.push_mode(Mode::ReplaceQueue);
            }
            browse::Action::Play(ids, how) => self.enqueue(ids, how),
            browse::Action::Favourite(id) => {
                self.toggle_favourite(id);
                // A shelf's tracks lose what was just let go.
                self.reload_shelves();
            }
            browse::Action::OpenAlbum(id) => self.go_page(Page::Album(id)),
            browse::Action::OpenArtist(id) => self.go_page(Page::Artist(id)),
            browse::Action::OpenCredited(of, id) => self.open_credited(of, id),
            browse::Action::Shuffle(ids) => self.enqueue(ids, PickerAction::Shuffle),
            browse::Action::ToggleAlbum(id) => {
                self.toggle_collection_favourite(koan_core::helpers::FavouriteKind::Album, id)
            }
            browse::Action::ToggleArtist(id) => {
                self.toggle_collection_favourite(koan_core::helpers::FavouriteKind::Artist, id)
            }
            browse::Action::Info(id) => self.open_track_details(id),
            browse::Action::ForgetPlays => self.history_page.forget_chosen(),
            browse::Action::Choose(ids) => self.open_chooser(ids),
            browse::Action::OpenFilters => {
                let lossless = self.browse().is_none_or(Browse::offers_lossless);
                self.filter_panel = Some(FilterPanel::new(self.browse_filters.clone(), lossless));
                self.push_mode(Mode::FilterPanel);
            }
            browse::Action::SeeAll(see) => {
                // The browse filters are shared, so the shelf's become the
                // standing ones, as in the apps.
                self.set_browse_filters(see.filter.clone());
                let page = match see.source {
                    Source::Artists => Page::Artists,
                    Source::Tracks => Page::Tracks,
                    _ => Page::Albums,
                };
                if let Some(target) = match see.source {
                    Source::Artists => Some(&mut self.artists_page),
                    Source::Tracks => Some(&mut self.tracks_page),
                    Source::Albums => Some(&mut self.albums_page),
                    Source::Artist(_) => None,
                } {
                    target.adopt(&see);
                }
                self.nav.go(page);
                if let Some(browse) = self.browse_mut() {
                    browse.ensure_loaded();
                }
            }
            browse::Action::Menu(title, items) => {
                let items = items
                    .into_iter()
                    .map(|(label, action)| (label, super::actions_menu::Item::Browse(action)))
                    .collect();
                self.open_actions_menu(title, items);
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
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let len = self.queue.vq_cache.entries.len();
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
            // `t` tags the row under the cursor, as in mutt.
            KeyCode::Char('t') if self.page() == Page::Queue => {
                let cursor = self.queue.cursor;
                self.toggle_selection(cursor);
            }
            KeyCode::Char('>') | KeyCode::Char('n') => self.skip(PlayerCommand::NextTrack),
            KeyCode::Char('<') => self.skip(PlayerCommand::PrevTrack),
            KeyCode::Char('s') => self.toggle_shuffle(),
            KeyCode::Char('T') => self.cycle_sleep(),
            KeyCode::Char('R') => self.cycle_repeat(),
            KeyCode::Char('.') | KeyCode::Right => {
                self.seek_to(self.state.position_ms().saturating_add(10_000));
            }
            KeyCode::Char(',') | KeyCode::Left => {
                self.seek_to(self.state.position_ms().saturating_sub(10_000));
            }
            KeyCode::Char('p') => {
                self.open_picker(PickerKind::Track);
            }
            KeyCode::Char('a') if ctrl => self.select_all(),
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
                let link = key.code == KeyCode::Char('O');
                let db_id = self
                    .visible_queue()
                    .get(self.queue.cursor)
                    .and_then(|e| e.db_id);
                self.open_entry_link(db_id, link);
            }
            KeyCode::Char('L') => self.lyrics_panel = !self.lyrics_panel,
            KeyCode::Char('V') => {
                self.viz_picker = Some(super::viz_picker::VizPickerState::new(
                    self.visualizer.mode,
                    self.viz_config.enabled,
                ));
                // Full screen while it is open, so the preview is seen.
                self.viz_fullscreen = true;
                self.push_mode(Mode::VizPicker);
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
                self.persist(move |cfg| cfg.visualizer.matrix_overlay = overlay);
            }
            KeyCode::Char('S') => {
                self.visualizer.bass_shake = !self.visualizer.bass_shake;
                self.say(if self.visualizer.bass_shake {
                    "bass shake on"
                } else {
                    "bass shake off"
                });
                let shake = self.visualizer.bass_shake;
                self.persist(move |cfg| cfg.visualizer.bass_shake = shake);
            }
            // The cursor and the selection, always live: Shift extends.
            KeyCode::Up | KeyCode::Char('k') if len > 0 => {
                let to = self.queue.cursor.checked_sub(1).unwrap_or(len - 1);
                self.move_cursor(to, shift);
            }
            KeyCode::Down | KeyCode::Char('j') if len > 0 => {
                let to = (self.queue.cursor + 1) % len;
                self.move_cursor(to, shift);
            }
            // The selection, or the track under the cursor, moved a place.
            KeyCode::Char('K') => self.move_selected(false),
            KeyCode::Char('J') => self.move_selected(true),
            KeyCode::PageUp => {
                let page = self.queue_visible_height().max(1);
                self.move_cursor(self.queue.cursor.saturating_sub(page), shift);
            }
            KeyCode::Char('u') if ctrl => {
                let page = self.queue_visible_height().max(1);
                self.move_cursor(self.queue.cursor.saturating_sub(page), false);
            }
            KeyCode::PageDown => {
                let page = self.queue_visible_height().max(1);
                self.move_cursor(self.queue.cursor + page, shift);
            }
            KeyCode::Char('d') if ctrl => {
                let page = self.queue_visible_height().max(1);
                self.move_cursor(self.queue.cursor + page, false);
            }
            KeyCode::Char('d') | KeyCode::Delete | KeyCode::Backspace => self.delete_selected(),
            KeyCode::Home => self.move_cursor(0, shift),
            KeyCode::End => self.move_cursor(usize::MAX, shift),
            KeyCode::Char('g') => self.move_cursor(0, false),
            KeyCode::Char('G') => self.move_cursor(usize::MAX, false),
            KeyCode::Char('m') if self.page() == Page::Queue => self.open_context_menu(None, false),
            KeyCode::Char('y') if ctrl => {
                self.tx.send(PlayerCommand::Redo).ok();
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
            KeyCode::Char('C') => self.open_play_on(),
            KeyCode::Char('E') => self.open_eq_page(),
            KeyCode::Char('w') => {
                let ids = self.queue_choice();
                self.open_chooser(ids);
            }
            KeyCode::Char('W') => self.queue_control(queue::Control::Save),
            KeyCode::Char('v') if self.page() == Page::Queue => {
                self.queue_control(queue::Control::Grouped(self.queue.flat));
            }
            KeyCode::Char('o') if self.page() == Page::Queue => {
                self.queue_control(queue::Control::Playing);
            }
            KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.queue_control(queue::Control::ClearQueue);
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
                // Woken off the lane: keys meanwhile are for the music where
                // it is.
                let wake = choice.id.clone();
                self.run_control_after(
                    move || koan_core::remote::control::wake_for_move(wake.as_deref()),
                    said,
                    move |asked, tx, state| {
                        koan_core::remote::control::move_music(choice.id, asked, tx, state, None)
                            .map(|_| ())
                    },
                );
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

    /// `run_control`, once `before` has run on a thread of its own: what has
    /// to happen first without holding up the commands sent meanwhile.
    fn run_control_after<T: Send + 'static>(
        &mut self,
        before: impl FnOnce() -> T + Send + 'static,
        said: String,
        work: impl FnOnce(T, &Sender<PlayerCommand>, &SharedPlayerState) -> Result<(), String>
        + Send
        + 'static,
    ) {
        let (lane, local, tx, report) = (
            self.lane.clone(),
            self.view.local.clone(),
            self.local_tx.clone(),
            self.task_tx.clone(),
        );
        let _ = std::thread::Builder::new()
            .name("koan-before-control".into())
            .spawn(move || {
                let ready = before();
                lane.run(move || {
                    let line = match work(ready, &tx, &local) {
                        Ok(()) => said,
                        Err(e) => e,
                    };
                    report.send(super::tasks::Report::Work(line)).ok();
                });
            });
    }

    fn open_play_on(&mut self) {
        self.play_on = Some(super::control::PlayOnState::new(&self.view.local));
        self.push_mode(Mode::PlayOn);
    }

    fn toggle_shuffle(&mut self) {
        let on = !self.state.play_mode().shuffle;
        self.tx.send(PlayerCommand::SetShuffle(on)).ok();
        self.say(if on { "shuffle on" } else { "shuffle off" });
    }

    /// Off, then each choice in turn, then off again.
    fn cycle_sleep(&mut self) {
        use koan_core::player::state::{Sleep, SleepTimer};
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
            Some(SleepTimer::EndOfRecord) => "sleep at the end of the album".into(),
        });
    }

    fn cycle_repeat(&mut self) {
        let repeat = self.state.play_mode().repeat.cycled();
        self.tx.send(PlayerCommand::SetRepeat(repeat)).ok();
        self.say(format!("repeat {}", repeat.as_str()));
    }

    /// What a click on a control of the transport or Now playing does.
    fn press(&mut self, control: Control) {
        let playing = self.state.track_info().and_then(|info| {
            self.queue
                .vq_cache
                .entries
                .iter()
                .find(|e| e.id == info.id)
                .and_then(|e| e.db_id)
        });
        match control {
            Control::PlayPause => self.toggle_pause(),
            Control::Previous => self.skip(PlayerCommand::PrevTrack),
            Control::Next => self.skip(PlayerCommand::NextTrack),
            Control::Shuffle => self.toggle_shuffle(),
            Control::Repeat => self.cycle_repeat(),
            Control::Sleep => self.cycle_sleep(),
            Control::Favourite => {
                if let Some(id) = playing {
                    self.toggle_favourite(id);
                }
            }
            Control::Output => {
                if self.this_device_shown() {
                    self.open_device_selector();
                }
            }
            Control::PlayOn => self.open_play_on(),
            Control::Eq => self.open_dsp_selector(),
            Control::Cover => {
                if self.art.now_playing_art.cached().is_some() {
                    self.push_mode(Mode::CoverArtZoom);
                }
            }
        }
    }

    fn open_device_selector(&mut self) {
        self.device_selector = Some(super::device_selector::OutputPicker::open(&self.state));
        self.push_mode(Mode::DeviceSelector);
    }

    /// Follow renderers coming and going while the picker is open.
    fn refresh_device_selector(&mut self) {
        if let Some(picker) = self.device_selector.as_mut() {
            picker.refresh(&self.state);
        }
    }

    fn handle_device_selector_key(&mut self, key: KeyEvent) {
        let Some(picker) = self.device_selector.as_mut() else {
            self.pop_mode();
            return;
        };
        let picked = picker.handle_key(key);
        self.apply_output(picked);
    }

    /// Does what the output picker asked.
    fn apply_output(&mut self, picked: super::device_selector::Picked) {
        use super::device_selector::Picked;
        match picked {
            Picked::Nothing => {}
            Picked::Close => {
                self.device_selector = None;
                self.pop_mode();
            }
            Picked::OpenEq => {
                self.device_selector = None;
                self.pop_mode();
                self.open_eq_page();
            }
            Picked::Volume(v) => {
                self.tx.send(PlayerCommand::SetRendererVolume(v)).ok();
            }
            Picked::Choose(output) => {
                if let Some(p) = self.device_selector.as_mut() {
                    p.chose(output.clone());
                }
                // Taken now, in the order picked: opening a renderer's
                // session is a few round trips, and a later pick drops it.
                let choice = koan_core::upnp::choose();
                let tx = self.tx.clone();
                std::thread::Builder::new()
                    .name("koan-output".into())
                    .spawn(move || {
                        if let Err(e) = koan_core::remote::outputs::set(output, choice, &tx) {
                            log::error!("output: {e}");
                        }
                    })
                    .ok();
            }
        }
    }

    /// The EQ page, for this device's output.
    pub fn open_eq_page(&mut self) {
        if self.this_device_shown() {
            self.go_page(Page::Eq);
        }
    }

    /// The preset menu, as the apps' transport has: Flat, each preset, the
    /// one the output in use was set from marked, and the EQ page.
    pub fn open_dsp_selector(&mut self) {
        if !self.this_device_shown() {
            return;
        }
        match self.eq_page.snap.as_deref() {
            Some(snap) => {
                self.dsp_selector = Some(super::dsp_selector::DspSelectorState::load(snap));
                self.push_mode(Mode::DspSelector);
            }
            None => self.say("the eq is still being read"),
        }
    }

    fn apply_dsp(&mut self, idx: usize) {
        use super::dsp_selector::Choose;
        let Some(choose) = self
            .dsp_selector
            .as_ref()
            .and_then(|s| s.entries.get(idx))
            .map(|e| e.choose.clone())
        else {
            return;
        };
        match choose {
            Choose::Edit => {
                self.dsp_selector = None;
                self.pop_mode();
                self.open_eq_page();
            }
            Choose::Preset(name) => {
                self.eq_page.use_preset(name);
                if let Some(sel) = self.dsp_selector.as_mut() {
                    sel.mark(idx);
                }
            }
            Choose::Nothing => {}
        }
    }

    /// Bypasses the EQ of the output in use, keeping what it played as a
    /// preset, or puts back what was bypassed.
    fn toggle_eq(&mut self) {
        if !self.this_device_shown() {
            return;
        }
        if let Some(said) = self.eq_page.bypass() {
            self.say(said);
        }
    }

    fn handle_dsp_selector_key(&mut self, key: KeyEvent) {
        let Some(sel) = self.dsp_selector.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.dsp_selector = None;
                self.pop_mode();
            }
            KeyCode::Char('E') | KeyCode::Char('e') => {
                self.dsp_selector = None;
                self.pop_mode();
                self.open_eq_page();
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
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('V') => {
                // Back to what was shown before the picker previewed others.
                let (mode, on) = (picker.current_mode, picker.was_on);
                self.viz_picker = None;
                self.viz_fullscreen = false;
                self.pop_mode();
                self.visualizer.mode = mode;
                self.viz_config.enabled = on;
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::Down | KeyCode::Char('j') => {
                let up = matches!(key.code, KeyCode::Up | KeyCode::Char('k'));
                picker.move_by(if up { -1 } else { 1 });
                // Live preview.
                match picker.selected() {
                    Some(mode) => {
                        self.visualizer.mode = mode;
                        self.viz_config.enabled = true;
                    }
                    None => self.viz_config.enabled = false,
                }
            }
            KeyCode::Enter => {
                let selected = picker.selected();
                self.viz_picker = None;
                self.viz_fullscreen = false;
                self.pop_mode();
                match selected {
                    Some(mode) => self.apply_viz_mode(mode),
                    None => {
                        self.viz_config.enabled = false;
                        self.say("visualiser off");
                        self.persist(|cfg| cfg.visualizer.enabled = false);
                    }
                }
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
        self.persist(move |cfg| {
            cfg.visualizer.mode = mode_str;
            cfg.visualizer.enabled = true;
        });
    }

    fn handle_context_menu_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.context_menu = None;
                self.pop_mode();
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::Down | KeyCode::Char('j') => {
                let delta = if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) {
                    -1
                } else {
                    1
                };
                if let Some(menu) = self.context_menu.as_mut() {
                    menu.cursor = super::context_menu::step(&menu.actions, menu.cursor, delta);
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
                    if let Some((action, _, _)) = menu
                        .actions
                        .iter()
                        .find(|(a, _, h)| *h == c && *a != ContextAction::Separator)
                    {
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

    /// The queue's menu for the selection, or the track under the cursor,
    /// opened at `at` or centred; `heading` when it is for an album heading.
    pub(super) fn open_context_menu(&mut self, at: Option<(u16, u16)>, heading: bool) {
        use super::context_menu::MenuFor;
        let ids = self.queue_choice();
        let favourite = !ids.is_empty() && ids.iter().all(|id| self.favourites.contains(id));
        let of = if heading {
            MenuFor::Album
        } else if self.queue.selected_ids.len() > 1 {
            MenuFor::Tracks { favourite }
        } else {
            let on_disk = ids
                .first()
                .and_then(|id| self.queue.facts.sources.get(id))
                .is_some_and(|&(_, on_disk)| on_disk);
            MenuFor::Track {
                in_library: !ids.is_empty(),
                favourite,
                on_disk,
            }
        };
        let cfg = koan_core::config::Config::cached();
        let share = cfg.remote.enabled && koan_core::helpers::remote_credential(&cfg).is_some();
        self.context_menu = Some(ContextMenuState {
            actions: super::context_menu::queue_items(of, share),
            cursor: 0,
            at,
        });
        self.push_mode(Mode::ContextMenu);
    }

    fn execute_context_action(&mut self, action: ContextAction) {
        self.pop_mode();
        match action {
            ContextAction::Separator => {}
            ContextAction::Play => {
                self.play_at_cursor();
            }
            ContextAction::Remove => {
                self.delete_selected();
            }
            ContextAction::ToggleFavourite => {
                // All of them become favourites, unless all of them were.
                let ids = self.queue_choice();
                let on = !ids.iter().all(|id| self.favourites.contains(id));
                for id in ids {
                    if self.favourites.contains(&id) != on {
                        self.toggle_favourite(id);
                    }
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
            ContextAction::GoToAlbum
            | ContextAction::GoToArtist
            | ContextAction::GoToAlbumArtist => {
                let db_id = self
                    .visible_queue()
                    .get(self.queue.cursor)
                    .and_then(|e| e.db_id);
                match action {
                    ContextAction::GoToAlbum => self.open_entry_link(db_id, true),
                    ContextAction::GoToArtist => self.open_entry_link(db_id, false),
                    _ => self.open_credited_artist(db_id, true),
                }
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
            KeyCode::Left => picker.move_tab(-1),
            KeyCode::Right => picker.move_tab(1),
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

    /// Acts on what the picker has chosen, and closes it: a track to jump to
    /// in the queue, or the picks for the queue, albums and artists standing
    /// for their tracks in the order picked.
    fn confirm_picker(&mut self, action: PickerAction) {
        let Some(picker) = self.picker.as_ref() else {
            return;
        };
        let (kind, picks) = (picker.kind(), picker.confirm());
        self.close_picker();
        if kind != PickerKind::QueueJump {
            if !picks.is_empty() {
                self.spawn_enqueue(action, move || {
                    use koan_core::db::queries;
                    let Ok(db) = koan_core::db::pool::shared().get() else {
                        return Vec::new();
                    };
                    picks
                        .into_iter()
                        .flat_map(|(kind, id)| match kind {
                            PickerKind::Album => queries::tracks_for_album(&db.conn, id)
                                .unwrap_or_default()
                                .iter()
                                .map(|t| t.id)
                                .collect(),
                            PickerKind::Artist => queries::tracks_for_artist(&db.conn, id)
                                .unwrap_or_default()
                                .iter()
                                .map(|t| t.id)
                                .collect(),
                            PickerKind::Track | PickerKind::QueueJump => vec![id],
                        })
                        .collect()
                });
            }
            return;
        }
        let Some(&(_, idx)) = picks.first() else {
            return;
        };
        let idx = idx as usize;
        if let Some(entry) = self.queue.vq_cache.entries.get(idx) {
            self.tx.send(PlayerCommand::Play(entry.id)).ok();
            self.queue.cursor = idx;
            // The jumped-to track near the top.
            self.queue.scroll_offset =
                queue::scroll_cursor_to_top(&self.queue.layout, idx, self.queue_visible_height());
        }
    }

    /// Puts library tracks in the queue as `how` says, on a thread of their
    /// own: each is read from the library first.
    pub(super) fn enqueue(&mut self, ids: Vec<i64>, how: PickerAction) {
        if !ids.is_empty() {
            self.spawn_enqueue(how, move || ids);
        }
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
        let scroll = self.track_details.as_mut().map(|d| &mut d.scroll);
        match key.code {
            KeyCode::Esc | KeyCode::Char('i') | KeyCode::Char('q') => self.close_info(),
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(s) = scroll {
                    *s = s.saturating_sub(1);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(s) = scroll {
                    *s += 1;
                }
            }
            KeyCode::PageUp => {
                if let Some(s) = scroll {
                    *s = s.saturating_sub(10);
                }
            }
            KeyCode::PageDown => {
                if let Some(s) = scroll {
                    *s += 10;
                }
            }
            _ => {}
        }
    }

    fn close_info(&mut self) {
        self.track_details = None;
        self.art.cover_art.clear();
        self.pop_mode();
    }

    /// Shows what the library holds of `track_id`.
    fn open_track_details(&mut self, track_id: i64) {
        match super::track_info::Details::load(track_id) {
            Some(details) => {
                self.track_details = Some(Box::new(details));
                self.push_mode(Mode::TrackDetails);
            }
            None => self.say("that track is no longer in the library"),
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
        let (id, track, path) = (entry.id, entry.db_id, entry.path.clone());
        self.info_arriving = arriving(entry);
        let details = track
            .and_then(super::track_info::Details::load)
            .unwrap_or_default();
        self.track_details = Some(Box::new(details));
        self.push_mode(Mode::TrackInfo(id));
        self.art.cover_art.want(&path, self.info_arriving, || track);
    }

    /// Opens the album, or the artist, of the library track `db_id`.
    pub fn open_entry_link(&mut self, db_id: Option<i64>, album_page: bool) {
        if album_page {
            self.open_track_page(db_id, |_, album| Page::Album(album.id));
        } else {
            self.open_credited_artist(db_id, false);
        }
    }

    /// Opens a menu, its items going to a credit's artists resolved into one
    /// item per artist.
    fn open_actions_menu(
        &mut self,
        title: String,
        items: Vec<(String, super::actions_menu::Item)>,
    ) {
        let credits = items.iter().any(|(_, i)| {
            matches!(
                i,
                super::actions_menu::Item::Browse(browse::Action::OpenCredited(..))
            )
        });
        let db = credits
            .then(|| koan_core::db::pool::shared().get().ok())
            .flatten();
        let items = super::actions_menu::resolve_credits(items, |of, id| {
            db.as_ref()
                .map(|db| super::actions_menu::credited(db, of, id))
                .unwrap_or_default()
        });
        drop(db);
        self.actions_menu = Some(ActionsMenu::new(title, items));
        self.push_mode(Mode::ActionsMenu);
    }

    /// Opens the page of the artist a library track's credit names, or its
    /// album's credit when `album` or when the track is filed under no
    /// artist.
    pub fn open_credited_artist(&mut self, db_id: Option<i64>, album: bool) {
        use koan_core::db::queries::{self, CreditsOf};
        let Some(db) = koan_core::db::pool::shared().get().ok() else {
            self.say("the library could not be opened");
            return;
        };
        let Some(track) = db_id.and_then(|id| queries::get_track_row(&db.conn, id).ok().flatten())
        else {
            self.say("that track is not in the library");
            return;
        };
        let artists = match (album || track.artist_id.is_none(), track.album_id) {
            (false, _) => super::actions_menu::credited(&db, CreditsOf::Tracks, track.id),
            (true, Some(a)) => super::actions_menu::credited(&db, CreditsOf::Albums, a),
            (true, None) => Vec::new(),
        };
        drop(db);
        self.go_to_artists(artists);
    }

    /// Opens the page of the artist a track's or album's credit names.
    pub fn open_credited(&mut self, of: koan_core::db::queries::CreditsOf, id: i64) {
        let Some(db) = koan_core::db::pool::shared().get().ok() else {
            self.say("the library could not be opened");
            return;
        };
        let artists = super::actions_menu::credited(&db, of, id);
        drop(db);
        self.go_to_artists(artists);
    }

    /// Opens the one artist's page, or asks which of several.
    fn go_to_artists(&mut self, artists: Vec<(i64, String)>) {
        match artists.as_slice() {
            [] => self.say("no artist to go to"),
            [(artist, _)] => self.go_page(Page::Artist(*artist)),
            _ => self.apply_browse(browse::Action::Menu(
                "Go to artist".into(),
                super::actions_menu::go_to_artists(&artists),
            )),
        }
    }

    /// Opens the page `to` picks from a library track and its album.
    fn open_track_page(
        &mut self,
        db_id: Option<i64>,
        to: impl FnOnce(&koan_core::db::queries::TrackRow, &koan_core::db::queries::AlbumRow) -> Page,
    ) {
        let found = db_id.and_then(|id| {
            let db = koan_core::db::pool::shared().get().ok()?;
            let track = koan_core::db::queries::get_track_row(&db.conn, id)
                .ok()
                .flatten()?;
            let album = koan_core::db::queries::get_album(&db.conn, track.album_id?)
                .ok()
                .flatten()?;
            Some(to(&track, &album))
        });
        match found {
            Some(page) => self.go_page(page),
            None => {
                self.say("that track is not in the library");
            }
        }
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

    /// Moves the selection, or the track under the cursor, one row down or
    /// up, and the cursor with it. A selection with gaps closes up around its
    /// first track.
    fn move_selected(&mut self, down: bool) {
        if self.queue.selected_ids.is_empty() {
            self.select_single(self.queue.cursor);
        }
        let entries = &self.queue.vq_cache.entries;
        let selected = &self.queue.selected_ids;
        let Some(first) = entries.iter().position(|e| selected.contains(&e.id)) else {
            return;
        };
        let ahead = entries
            .get(self.queue.cursor)
            .filter(|e| selected.contains(&e.id))
            .map(|_| {
                entries[first..self.queue.cursor]
                    .iter()
                    .filter(|e| selected.contains(&e.id))
                    .count()
            });
        let start = if down {
            first + 1
        } else if let Some(up) = first.checked_sub(1) {
            up
        } else {
            return;
        };
        if let Some(start) = self.place_selected(start)
            && let Some(ahead) = ahead
        {
            self.queue.cursor = start + ahead;
        }
        self.update_scroll();
    }

    /// Moves the selected tracks so that, together, they start at `start`;
    /// where they start, once there is room for them, if they moved.
    pub(super) fn place_selected(&mut self, start: usize) -> Option<usize> {
        let entries = &self.queue.vq_cache.entries;
        let selected = &self.queue.selected_ids;
        let cmd = queue::placement(entries, selected, start)?;
        let moving = entries.iter().filter(|e| selected.contains(&e.id)).count();
        self.tx.send(cmd).ok();
        Some(start.min(entries.len() - moving))
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

    pub fn open_library(&mut self) {
        self.go_page(Page::Artists);
    }

    /// Opens the picker on `kind`'s tab; each tab is read on a thread of its
    /// own and fills in when it arrives.
    fn open_picker(&mut self, kind: PickerKind) {
        self.picker = Some(PickerState::library(kind));
        self.push_mode(Mode::Picker(kind));
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
                let credits = self.queue.facts.credits_of(e);
                let has_artist = !credits.artist.is_empty() && credits.differ;
                let display = if has_artist {
                    format!("{} \u{2014} {}", credits.artist, e.title)
                } else {
                    e.title.clone()
                };
                let parts = if has_artist {
                    vec![
                        (credits.artist.clone(), PickerPartKind::Artist),
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

    /// Puts the queue's cursor on `to`. With `extend` the selection runs
    /// from its anchor to there; otherwise it is left as it is, and the
    /// cursor's row is where the next extension starts.
    fn move_cursor(&mut self, to: usize, extend: bool) {
        let len = self.queue.vq_cache.entries.len();
        if len == 0 {
            return;
        }
        self.queue.cursor = to.min(len - 1);
        if extend {
            self.extend_selection_to(self.queue.cursor);
        } else {
            self.queue.anchor_id = self
                .queue
                .vq_cache
                .entries
                .get(self.queue.cursor)
                .map(|e| e.id);
        }
        self.update_scroll();
    }

    /// Rows of queue content on screen. `Borders::TOP` eats the first row.
    ///
    /// Rendering, scrolling and every hit-test read this — they drift apart the
    /// moment there is more than one definition.
    pub(super) fn queue_visible_height(&self) -> usize {
        self.layout.queue_area.height as usize
    }

    fn update_scroll(&mut self) {
        let visible_height = self.queue_visible_height();
        self.queue.scroll_offset = queue::scroll_for_cursor(
            &self.queue.layout,
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
            self.queue.facts = queue::Facts::read(&self.queue.vq_cache.entries, &self.state);
            self.relayout_queue();

            self.clamp_queue_cursor();
            // A queue that shrank under the scroll would draw its last line
            // alone at the top, and clicks would land on rows not drawn.
            let max = queue::display_line_count(&self.queue.layout)
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

    /// Does what a control in the queue's toolbar is for.
    fn queue_control(&mut self, control: queue::Control) {
        match control {
            queue::Control::Playing => {
                if let Some(idx) = self
                    .queue
                    .vq_cache
                    .entries
                    .iter()
                    .position(|e| e.status == QueueEntryStatus::Playing)
                {
                    let height = self.queue_visible_height();
                    self.queue.scroll_offset =
                        queue::scroll_cursor_to_top(&self.queue.layout, idx, height);
                }
            }
            queue::Control::Grouped(grouped) => {
                self.queue.flat = !grouped;
                self.relayout_queue();
                self.update_scroll();
                self.persist(move |cfg| cfg.playback.queue_grouped = grouped);
            }
            queue::Control::Undo => {
                self.tx.send(PlayerCommand::Undo).ok();
            }
            queue::Control::Redo => {
                self.tx.send(PlayerCommand::Redo).ok();
            }
            queue::Control::Save => {
                let ids: Vec<i64> = self
                    .visible_queue()
                    .iter()
                    .filter_map(|e| e.db_id)
                    .collect();
                if ids.is_empty() {
                    self.say("nothing in the queue is in the library");
                } else {
                    self.open_name_prompt(Purpose::New(ids), "queue".into());
                }
            }
            queue::Control::ClearQueue => {
                self.tx.send(PlayerCommand::ClearPlaylist).ok();
                self.queue.selected_ids.clear();
                self.say("cleared the queue: ctrl+z puts it back");
            }
            queue::Control::RemoveSelected => self.delete_selected(),
        }
    }

    /// Lays the queue's lines out again, grouped or a row per track.
    pub(super) fn relayout_queue(&mut self) {
        self.queue.layout = queue::Layout::build(
            &self.queue.vq_cache.entries,
            !self.queue.flat,
            &self.queue.facts,
        );
    }

    pub fn visible_queue(&self) -> &[QueueEntry] {
        &self.queue.vq_cache.entries
    }
}

/// A write to the config, as the writer thread takes it.
type ConfigChange = Box<dyn FnOnce(&mut koan_core::config::Config) + Send>;

/// The thread that writes the config: one at a time, in the order asked,
/// so a later toggle is never overwritten by an earlier one landing after it.
fn config_writer() -> Sender<ConfigChange> {
    let (tx, rx) = crossbeam_channel::unbounded::<ConfigChange>();
    std::thread::Builder::new()
        .name("koan-config".into())
        .spawn(move || {
            for change in rx {
                if let Err(e) = koan_core::config::Config::persist(change) {
                    log::warn!("config: {e}");
                }
            }
        })
        .ok();
    tx
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
