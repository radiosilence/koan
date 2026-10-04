import KoanFFI
import SwiftUI
#if os(iOS)
import UIKit
#endif

/// Everything the app needs, built once the engine is up.
///
/// Constructing `KoanEngine` spawns the player thread and opens the library, so
/// it happens once and is handed down rather than being reachable globally.
@MainActor
@Observable
final class AppState {
    let engine: KoanEngine
    /// The engine's state, mirrored. Everything that reads it reads this.
    let mirror: EngineMirror
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
        // The name the person gave the phone needs Apple's
        // user-assigned-device-name entitlement; without it this is "iPhone".
        // A Mac names itself by its hostname.
        #if os(iOS)
        let engine = try await KoanEngine(deviceName: UIDevice.current.name)
        #else
        let engine = try await KoanEngine(deviceName: nil)
        #endif
        self.engine = engine
        let mirror = EngineMirror()
        self.mirror = mirror
        // Before anything else asks the engine a question: the first batch is
        // the whole state, so the first frame draws against something real.
        mirror.start(engine: engine)
        let player = PlayerModel(engine: engine, mirror: mirror)
        self.player = player
        let library = LibraryModel(engine: engine)
        self.library = library
        let nav = Navigator(library: library)
        self.nav = nav
        self.search = SearchModel(engine: engine, nav: nav)
        let art = CoverArtCache(engine: engine)
        self.art = art
        self.organize = OrganizeModel(engine: engine)
        let playlists = PlaylistsModel(engine: engine)
        self.playlists = playlists
        let activity = ActivityModel()
        self.activity = activity
        self.levels = PlayingLevels(engine: engine)
        library.activity = activity
        library.art = art
        library.mirror = mirror
        playlists.mirror = mirror
        nav.playlists = playlists
        player.activity = activity
        organize.activity = activity
        // Playlist failures go where every other engine failure goes rather
        // than into a modal of their own.
        playlists.report = { [weak player] message in player?.lastError = message }

        activity.cancelLibraryTask = { engine.cancelLibraryTask() }

        // The engine syncs and scans on its own — on startup, on a timer, and
        // when the library folders change. Those are the slow things a user is
        // most likely to notice and least likely to have asked for, so they get
        // a row like anything else. Followed rather than polled: the engine
        // says whether each is running, in the same stream as everything else.
        mirror.follow { [weak activity, weak mirror] in
            guard let activity, let mirror else { return }
            activity.mirror(
                "Syncing with server", uses: [.remoteTracks], followsSync: true,
                running: mirror.tasks.syncing)
            activity.showSync(mirror.syncProgress)
            activity.mirror(
                "Scanning library", uses: .localLibrary, cancellable: true,
                running: mirror.tasks.scanning)
        }


        let centre = NowPlayingCentre(player: player, mirror: mirror, art: art)
        self.nowPlaying = centre

        // Single-key shortcuts, caught before the focused list eats them.
        #if os(macOS)
        self.hotkeys = Hotkeys.standard(player: player, library: library, nav: nav, ui: ui)
        FullScreenBackstop.install()
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

    /// Everything that has to happen once, after the engine is up.
    ///
    /// Here rather than in a scene root, so that every shell gets it: none of it
    /// is about a window.
    func start() async {
        await player.start()
        await player.restoreSession()
    }
}
