import KoanFFI
import SwiftUI

/// Everything the app needs, built once the engine is up.
///
/// Constructing `KoanEngine` spawns the player thread and opens the library, so
/// it happens once and is handed down rather than being reachable globally.
@MainActor
@Observable
final class AppState {
    let engine: KoanEngine
    let player: PlayerModel
    let library: LibraryModel
    let nav: Navigator
    let search: SearchModel
    let art: CoverArtCache
    let organize: OrganizeModel
    let playlists: PlaylistsModel
    let activity: ActivityModel
    let levels: PlayingLevels
    let ui = UIState()
    /// Menu enablement and single-key shortcuts — both are the menu bar's, and
    /// there isn't one on iOS.
    #if os(macOS)
    let textFocus = TextFocus()
    let hotkeys: Hotkeys
    #endif
    private var nowPlaying: NowPlayingCentre?

    init() async throws {
        let engine = try await KoanEngine()
        self.engine = engine
        let player = PlayerModel(engine: engine)
        self.player = player
        let library = LibraryModel(engine: engine)
        self.library = library
        let nav = Navigator(library: library)
        self.nav = nav
        self.search = SearchModel(engine: engine, library: library, nav: nav)
        let art = CoverArtCache(engine: engine)
        self.art = art
        self.organize = OrganizeModel(engine: engine)
        let playlists = PlaylistsModel(engine: engine)
        self.playlists = playlists
        let activity = ActivityModel()
        self.activity = activity
        self.levels = PlayingLevels(engine: engine)
        library.activity = activity
        player.activity = activity
        organize.activity = activity
        playlists.activity = activity
        // Playlist failures go where every other engine failure goes rather
        // than into a modal of their own.
        playlists.report = { [weak player] message in player?.lastError = message }

        // The engine syncs and scans on its own — on startup, on a timer, and
        // when the library folders change. Those are the slow things a user is
        // most likely to notice and least likely to have asked for, so they get
        // a row like anything else.
        activity.mirror("Syncing with server") { engine.isAutoSyncing() }
        activity.mirror("Scanning library") { engine.isAutoScanning() }
        activity.cancelLibraryTask = { engine.cancelLibraryTask() }

        // A finished download changes the library's cached count and nothing
        // else would say so — the count is a database read, and the download
        // ran in the engine.
        player.onDownloadsLanded = { [weak library] in library?.loadStats() }

        // Control Center and the media keys ride the player's existing poll.
        let centre = NowPlayingCentre(player: player, art: art)
        self.nowPlaying = centre
        player.onTick = { [weak centre] in centre?.refresh() }

        // Single-key shortcuts, caught before the focused list eats them.
        #if os(macOS)
        self.hotkeys = Hotkeys.standard(player: player, library: library, nav: nav, ui: ui)
        #endif

        // A client that cannot reach its server fails at everything quietly:
        // nothing plays, nothing downloads, and every record comes back with no
        // artwork — which reads as an empty library rather than as being signed
        // out. The engine knows why; this is it saying so. Off the launch path,
        // since the answer can involve the credential store.
        Task { [weak player] in
            if let problem = await engine.remoteProblem() {
                player?.lastError = problem
            }
        }
    }
}
