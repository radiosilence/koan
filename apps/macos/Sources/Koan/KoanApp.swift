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
    let activity: ActivityModel
    let textFocus = TextFocus()
    let ui = UIState()
    let hotkeys: Hotkeys
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
        let activity = ActivityModel()
        self.activity = activity
        library.activity = activity
        player.activity = activity
        organize.activity = activity

        // The engine syncs and scans on its own — on startup, on a timer, and
        // when the library folders change. Those are the slow things a user is
        // most likely to notice and least likely to have asked for, so they get
        // a row like anything else.
        activity.mirror("Syncing with server") { engine.isAutoSyncing() }
        activity.mirror("Scanning library") { engine.isAutoScanning() }
        activity.cancelLibraryTask = { engine.cancelLibraryTask() }

        // Control Center and the media keys ride the player's existing poll.
        let centre = NowPlayingCentre(player: player, art: art)
        self.nowPlaying = centre
        player.onTick = { [weak centre] in centre?.refresh() }

        // Single-key shortcuts, caught before the focused list eats them.
        self.hotkeys = Hotkeys.standard(player: player, nav: nav, ui: ui)

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

@main
struct KoanApp: App {
    @State private var state: AppState?

    /// Someone is in a text field, so every shortcut whose key also means
    /// something while typing stands down. Read in the Scene body, so flipping
    /// it re-evaluates the menus — which is the point: a *disabled* menu item
    /// releases its key equivalent to the responder chain, and that is the only
    /// thing that hands the keystroke back to macOS.
    private var isTyping: Bool { state?.textFocus.isEditing == true }
    @Environment(\.scenePhase) private var scenePhase
    @State private var startupError: String?
    @AppStorage("showLyrics") private var showLyrics = false

    var body: some Scene {
        Window("koan", id: MainWindow.id) {
            Group {
                if let state {
                    RootView(hotkeys: state.hotkeys)
                        .environment(state)
                        .environment(state.ui)
                        .environment(state.player)
                        .environment(state.library)
                        .environment(state.nav)
                        .environment(state.search)
                        .environment(state.art)
                        .environment(state.organize)
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
                    let created = try await AppState()
                    await created.player.start()
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
            if phase != .active {
                Task { await state?.player.saveSession() }
            }
        }
        .windowToolbarStyle(.unified(showsTitle: false))
        .commands {
            CommandGroup(after: .newItem) {
                Button("Add Music…") { state?.ui.showingPicker = true }
                    .keyboardShortcut("k", modifiers: .command)
            }

            CommandGroup(replacing: .sidebar) {
                ForEach(NavigationCommand.all, id: \.section) { command in
                    Button(command.title) { state?.nav.show(command.section) }
                        .keyboardShortcut(command.key, modifiers: .command)
                }
                Divider()
                Button("Back") { state?.nav.goBack() }
                    .keyboardShortcut("[", modifiers: .command)
                    .disabled(isTyping)
                Button("Forward") { state?.nav.goForward() }
                    .keyboardShortcut("]", modifiers: .command)
                    .disabled(isTyping)
                Divider()
                Button("Toggle Lyrics") { showLyrics.toggle() }
                    .keyboardShortcut("l", modifiers: [.command, .option])
                Divider()
            }

            CommandMenu("Playback") {
                // No `.keyboardShortcut(.space)`: a focused list wins that
                // contest. Hotkeys handles the key; this stays for
                // discoverability and the menu shows the shortcut anyway.
                Button("Play / Pause") { state?.player.togglePlayPause() }
                // Arrow keys with a modifier are text navigation first: ⌘← is
                // start-of-line, ⌥← is previous word. Disabled rather than
                // declined — a disabled item releases its key equivalent, and
                // that is the only way the field ever sees it.
                Button("Next") { state?.player.next() }
                    .keyboardShortcut(.rightArrow, modifiers: .command)
                    .disabled(isTyping)
                Button("Previous") { state?.player.previous() }
                    .keyboardShortcut(.leftArrow, modifiers: .command)
                    .disabled(isTyping)
                Divider()
                Button("Skip Forward") { state?.player.seek(bySeconds: 10) }
                    .keyboardShortcut(.rightArrow, modifiers: .option)
                    .disabled(isTyping)
                Button("Skip Back") { state?.player.seek(bySeconds: -10) }
                    .keyboardShortcut(.leftArrow, modifiers: .option)
                    .disabled(isTyping)
                Divider()
                Button("Favourite Current Track") { state?.player.toggleFavouriteCurrent() }
                    .keyboardShortcut("d", modifiers: .command)
                Button("Toggle Radio") { state?.player.toggleRadio() }
                    .keyboardShortcut("r", modifiers: [.command, .option])
            }

            // Replaces the stock Edit ▸ Undo, which has no undo manager behind
            // it here. Declaring ⌘Z anywhere else just loses to it.
            CommandGroup(replacing: .undoRedo) {
                // ⌘Z while typing is undoing the typing, not the queue — and
                // the field editor has its own undo stack to do it with.
                Button("Undo") { state?.player.undo() }
                    .keyboardShortcut("z", modifiers: .command)
                    .disabled(isTyping)
                Button("Redo") { state?.player.redo() }
                    .keyboardShortcut("z", modifiers: [.command, .shift])
                    .disabled(isTyping)
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

            // ⌘F means "narrow what I'm looking at" where there is a filter for
            // that, and the library lookup everywhere else.
            CommandGroup(after: .pasteboard) {
                Divider()
                Button("Find") {
                    guard let state else { return }
                    if state.nav.section?.filterPlaceholder != nil {
                        state.ui.focusFilter()
                    } else {
                        state.ui.focusSearch()
                    }
                }
                .keyboardShortcut("f", modifiers: .command)
            }

            CommandMenu("Queue") {
                Button("Save Session") { Task { await state?.player.saveSession() } }
                Button("Clear Queue") { state?.player.clearQueue() }
            }

            CommandGroup(replacing: .help) {
                Button("Keyboard Shortcuts") { state?.ui.showingShortcuts = true }
                    .keyboardShortcut("/", modifiers: .command)
            }

            CommandMenu("Library") {
                // Disabled while one is running: they all queue behind the same
                // database writer, so a second only makes both slower. Reads
                // `isLibraryBusy` rather than the task list, which changes on
                // every progress tick and would rebuild the menus with it.
                Group {
                    Button("Rescan Local Folders") { state?.library.scan() }
                        .keyboardShortcut("r", modifiers: [.command, .shift])
                    Button("Force Rescan") { state?.library.scan(force: true) }
                    Divider()
                    Button("Sync Remote Library") { state?.library.syncRemote() }
                    Button("Full Remote Sync") { state?.library.syncRemote(full: true) }
                }
                .disabled(state?.activity.isLibraryBusy ?? false)
                Divider()
                Button("Clear Artwork Cache") { state?.art.purge() }
            }
        }

        // A window, not a sheet. A sheet is not resizable — AppKit leaves the
        // style mask off and SwiftUI pins its content size — and this is a
        // table of file paths, which is exactly the thing someone wants to make
        // wider. A Window gets `.defaultSize`, a resize grip, and a size macOS
        // remembers between launches, none of which had to be written.
        //
        // It also means the library stays visible behind it, which suits a
        // preview you are checking rather than a prompt you are answering.
        Window("Organize Files", id: OrganizeWindow.id) {
            if let state {
                OrganizeWindow()
                    // A separate scene inherits nothing, so everything it reads
                    // is listed here — a missing one is not a compile error.
                    .environment(state.organize)
                    .environment(state.activity)
            }
        }
        .defaultSize(width: 940, height: 640)
        .keyboardShortcut(nil)

        Settings {
            if let state {
                SettingsView()
                    .environment(state.player)
                    .environment(state.library)
                    // A separate scene inherits nothing from the WindowGroup, so
                    // every environment value the settings window reads has to
                    // be listed here — and a missing one is not a compile error,
                    // it is a trap the first time the window opens.
                    .environment(state.activity)
                    .environment(state.art)
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
    let section: Navigator.Section

    static let all: [NavigationCommand] = [
        .init(title: "Queue", key: "1", section: .queue),
        .init(title: "Albums", key: "2", section: .albums),
        .init(title: "Artists", key: "3", section: .artists),
        .init(title: "Favourites", key: "4", section: .favourites),
        .init(title: "History", key: "5", section: .playHistory),
        .init(title: "Snapshots", key: "6", section: .snapshots),
    ]
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

/// The main window's scene id. SwiftUI puts it on the `NSWindow`, which is how
/// the single-key shortcuts tell koan's own window from a sheet or one of the
/// auxiliary scenes.
enum MainWindow {
    static let id = "main"
}
