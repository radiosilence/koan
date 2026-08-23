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
    let search: SearchModel
    let art: CoverArtCache
    let activity: ActivityModel
    private var nowPlaying: NowPlayingCentre?
    private var keys: KeyMonitor?

    init() throws {
        let engine = try KoanEngine()
        self.engine = engine
        let player = PlayerModel(engine: engine)
        self.player = player
        let library = LibraryModel(engine: engine)
        self.library = library
        self.search = SearchModel(engine: engine, library: library)
        let art = CoverArtCache(engine: engine)
        self.art = art
        let activity = ActivityModel()
        self.activity = activity
        library.activity = activity
        player.activity = activity

        // The engine syncs and scans on its own — on startup, on a timer, and
        // when the library folders change. Those are the slow things a user is
        // most likely to notice and least likely to have asked for, so they get
        // a row like anything else.
        activity.mirror("Syncing with server") { engine.isAutoSyncing() }
        activity.mirror("Scanning library") { engine.isAutoScanning() }

        // Control Center and the media keys ride the player's existing poll.
        let centre = NowPlayingCentre(player: player, art: art)
        self.nowPlaying = centre
        player.onTick = { [weak centre] in centre?.refresh() }

        // Space has to be caught before the focused list eats it.
        self.keys = KeyMonitor { [weak player] in player?.togglePlayPause() }
    }
}

@main
struct KoanApp: App {
    @State private var state: AppState?
    @Environment(\.scenePhase) private var scenePhase
    @State private var startupError: String?
    @State private var showingPicker = false
    @AppStorage("showLyrics") private var showLyrics = false

    private func toggleLyrics() { showLyrics.toggle() }

    var body: some Scene {
        Window("koan", id: "main") {
            Group {
                if let state {
                    RootView(showingPicker: $showingPicker)
                        .environment(state)
                        .environment(state.player)
                        .environment(state.library)
                        .environment(state.search)
                        .environment(state.art)
                        .environment(state.activity)
                        // One accent for the whole app, from the icon. Without
                        // this everything inherits the system blue.
                        .tint(.koanAccent)
                } else if let startupError {
                    StartupErrorView(message: startupError)
                } else {
                    ProgressView().controlSize(.small)
                }
            }
            .frame(minWidth: 940, minHeight: 620)
            .task {
                guard state == nil, startupError == nil else { return }
                do {
                    let created = try AppState()
                    created.player.start()
                    created.player.restoreSession()
                    created.library.loadInitial()
                    state = created
                } catch {
                    startupError = String(describing: error)
                }
            }
        }
        .onChange(of: scenePhase) { _, phase in
            // Backgrounding is the last dependable moment before termination.
            if phase != .active { state?.player.saveSession() }
        }
        .windowToolbarStyle(.unified(showsTitle: false))
        .commands {
            CommandGroup(after: .newItem) {
                Button("Add Music…") { showingPicker = true }
                    .keyboardShortcut("k", modifiers: .command)
            }

            CommandGroup(replacing: .sidebar) {
                ForEach(NavigationCommand.all, id: \.section) { command in
                    Button(command.title) { state?.library.section = command.section }
                        .keyboardShortcut(command.key, modifiers: .command)
                }
                Divider()
                Button("Back") { state?.library.goBack() }
                    .keyboardShortcut("[", modifiers: .command)
                Button("Forward") { state?.library.goForward() }
                    .keyboardShortcut("]", modifiers: .command)
                Divider()
                Button("Toggle Lyrics") { showLyrics.toggle() }
                    .keyboardShortcut("l", modifiers: [.command, .option])
                Divider()
            }

            CommandMenu("Playback") {
                // No `.keyboardShortcut(.space)`: a focused list wins that
                // contest. KeyMonitor handles the key; this stays for
                // discoverability and the menu shows the shortcut anyway.
                Button("Play / Pause") { state?.player.togglePlayPause() }
                Button("Next") { state?.player.next() }
                    .keyboardShortcut(.rightArrow, modifiers: .command)
                Button("Previous") { state?.player.previous() }
                    .keyboardShortcut(.leftArrow, modifiers: .command)
                Divider()
                Button("Skip Forward") { state?.player.seek(bySeconds: 10) }
                    .keyboardShortcut(.rightArrow, modifiers: .option)
                Button("Skip Back") { state?.player.seek(bySeconds: -10) }
                    .keyboardShortcut(.leftArrow, modifiers: .option)
                Divider()
                Button("Favourite Current Track") { state?.player.toggleFavouriteCurrent() }
                    .keyboardShortcut("d", modifiers: .command)
                Button("Toggle Radio") { state?.player.toggleRadio() }
                    .keyboardShortcut("r", modifiers: [.command, .option])
            }

            // Replaces the stock Edit ▸ Undo, which has no undo manager behind
            // it here. Declaring ⌘Z anywhere else just loses to it.
            CommandGroup(replacing: .undoRedo) {
                Button("Undo") { state?.player.undo() }
                    .keyboardShortcut("z", modifiers: .command)
                Button("Redo") { state?.player.redo() }
                    .keyboardShortcut("z", modifiers: [.command, .shift])
            }

            // The queue borrows these, but they must still mean the ordinary
            // thing while typing — ⌘A in the search field selects the text, not
            // the whole queue. EditCommands routes on what has focus.
            CommandGroup(replacing: .pasteboard) {
                Button("Cut") {
                    EditCommands.cut { state?.player.cutSelection() }
                }
                .keyboardShortcut("x", modifiers: .command)
                Button("Copy") {
                    EditCommands.copy { state?.player.copySelection() }
                }
                .keyboardShortcut("c", modifiers: .command)
                Button("Paste") {
                    EditCommands.paste { state?.player.paste() }
                }
                .keyboardShortcut("v", modifiers: .command)
                Button("Delete") {
                    EditCommands.delete { state?.player.removeSelected() }
                }
                .keyboardShortcut(.delete, modifiers: [])
                Divider()
                Button("Select All") {
                    EditCommands.selectAll { state?.player.selectAllQueue() }
                }
                .keyboardShortcut("a", modifiers: .command)
            }

            CommandMenu("Queue") {
                Button("Save Session") { state?.player.saveSession() }
                Button("Clear Queue") { state?.player.clearQueue() }
            }

            CommandMenu("Library") {
                Button("Rescan Local Folders") { state?.library.scan() }
                    .keyboardShortcut("r", modifiers: [.command, .shift])
                Button("Force Rescan") { state?.library.scan(force: true) }
                Divider()
                Button("Sync Remote Library") { state?.library.syncRemote() }
                Button("Full Remote Sync") { state?.library.syncRemote(full: true) }
                Divider()
                Button("Clear Artwork Cache") { state?.art.purge() }
            }
        }

        Settings {
            if let state {
                SettingsView()
                    .environment(state.player)
                    .environment(state.library)
            }
        }
    }
}

/// Menu commands must not *read* anything that changes often.
///
/// `.commands` is part of the Scene body, so reading an observable that ticks —
/// `isPlaying`, `radioEnabled` — makes SwiftUI rebuild every menu ten times a
/// second. That shows up as the Edit menu flickering, and as menu items and
/// keyboard shortcuts going dead because they are torn down mid-use. So the
/// titles here are fixed and the bodies only ever call methods.
///
/// Sections reachable from the View menu, in sidebar order.
private struct NavigationCommand {
    let title: String
    let key: KeyEquivalent
    let section: LibraryModel.Section

    static let all: [NavigationCommand] = [
        .init(title: "Queue", key: "1", section: .queue),
        .init(title: "Albums", key: "2", section: .albums),
        .init(title: "Artists", key: "3", section: .artists),
        .init(title: "Favourites", key: "4", section: .favourites),
        .init(title: "Snapshots", key: "5", section: .snapshots),
    ]
}

extension LibraryModel.Section: Identifiable {
    public var id: Self { self }
}

/// The library is a file on disk; if it can't be opened there is no app to show.
private struct StartupErrorView: View {
    let message: String

    var body: some View {
        VStack(spacing: 14) {
            Image(systemName: "exclamationmark.triangle")
                .font(.system(size: 34, weight: .light))
                .foregroundStyle(.orange)
            Text("Couldn't open your library")
                .font(.title3.weight(.medium))
            Text(message)
                .font(.callout)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .textSelection(.enabled)
            Text("Run `koan scan` to build one.")
                .font(.callout.monospaced())
                .foregroundStyle(.tertiary)
        }
        .padding(40)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
