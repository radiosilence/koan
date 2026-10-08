import KoanFFI
import SwiftUI

@main
struct KoanApp: App {
    @NSApplicationDelegateAdaptor private var delegate: AppDelegate
    /// Started by the delegate at launch rather than by the window, which a
    /// kōan resident in the menu bar may never open.
    private var state: AppState? { delegate.state }

    var body: some Scene {
        Window("kōan", id: MainWindow.id) {
            Group {
                if let state {
                    RootView(hotkeys: state.hotkeys)
                        .appEnvironment(state)
                        .modifier(InviteConfirmation(state: state))
                        .modifier(PairingConfirmation(state: state))
                        // One accent for the whole app, from the icon. Without
                        // this everything inherits the system blue.
                        .tint(.koanAccent)
                } else if let startupError = delegate.startupError {
                    StartupErrorView(message: startupError)
                } else {
                    ProgressView().controlSize(.small)
                }
            }
            // Room for all three columns at the width each one draws itself
            // at, whether or not the third is open.
            //
            // `NavigationSplitView` does not refuse to go below a column's
            // declared minimum. It lays the column out at its *ideal* width and
            // clips whatever does not fit, and with no slack left it stops
            // animating and starts clamping — which is one cause behind two
            // symptoms: the sidebar's rows hanging off the side of the window,
            // and the lyrics panel arriving in a single frame instead of
            // sliding. So the floor is the sum of what the columns draw at:
            // the widest page's stage (the record and playlist headers, ~760)
            // plus the sidebar's 215 and the inspector's 280. It stays under
            // 1280 so the window tiles to half of a 2560pt display.
            //
            // One number rather than one per column count: a floor that moved
            // when the lyrics panel opened resized the window under you, and a
            // window that jumps is worse than a window that is wide.
            .frame(minWidth: 1260, minHeight: 620)
            .onOpenURL { url in
                if let state { state.open(url: url) } else { delegate.pendingURL = url }
            }
        }
        .windowToolbarStyle(.unified(showsTitle: false))
        // A run with no one at the screen — the evidence renderer, or a
        // throwaway device for it to see — opens no window there.
        .defaultLaunchBehavior(Residency.windowless ? .suppressed : .automatic)
        // Menu commands must not *read* anything that changes often. `.commands`
        // is part of the Scene body, so reading an observable that ticks —
        // `isPlaying`, the queue — makes SwiftUI rebuild every menu ten times
        // a second: the Edit menu flickers, and menu items and keyboard shortcuts
        // go dead because they are torn down mid-use. So the titles here are
        // fixed and the bodies only ever call methods.
        .commands {
            CommandGroup(after: .newItem) {
                // ⌘K is the search everywhere else it exists, and koan's
                // search knows albums, artists and tracks — so it goes to the
                // field rather than to the sheet that builds a queue.
                ShortcutButton(.search) { state?.ui.focusSearch() }
                ShortcutButton(.addMusic) { state?.ui.showingPicker = true }
            }

            CommandGroup(replacing: .sidebar) {
                ForEach(NavigationCommand.all, id: \.section) { command in
                    ShortcutButton(command.shortcut) { state?.nav.show(command.section) }
                }
                Divider()
                ShortcutButton(.back) { state?.nav.goBack() }
                    .disabledWhileTyping(state?.textFocus)
                ShortcutButton(.forward) { state?.nav.goForward() }
                    .disabledWhileTyping(state?.textFocus)
                Divider()
                ShortcutButton(.lyrics) { state?.ui.toggleLyrics() }
                Divider()
            }

            CommandMenu("Playback") {
                // No `.keyboardShortcut(.space)`: a focused list wins that
                // contest. Hotkeys handles the key; this stays for
                // discoverability and the menu shows the shortcut anyway.
                Button { state?.player.togglePlayPause() } label: {
                    Label("Play / Pause", koan: Icon.playPause)
                }
                // Arrow keys with a modifier are text navigation first: ⌘← is
                // start-of-line, ⌥← is previous word. Disabled rather than
                // declined — a disabled item releases its key equivalent, and
                // that is the only way the field ever sees it.
                ShortcutButton(.next) { state?.player.next() }
                    .disabledWhileTyping(state?.textFocus)
                ShortcutButton(.previous) { state?.player.previous() }
                    .disabledWhileTyping(state?.textFocus)
                Divider()
                ShortcutButton(.skipForward) { state?.player.seek(bySeconds: 10) }
                    .disabledWhileTyping(state?.textFocus)
                ShortcutButton(.skipBack) { state?.player.seek(bySeconds: -10) }
                    .disabledWhileTyping(state?.textFocus)
                Divider()
                // Through the library, which is what every heart in the app
                // reads. Going straight to the engine would flip the row and
                // leave the UI showing the old answer.
                ShortcutButton(.favourite) {
                    guard let state, let trackId = state.player.currentTrackId else { return }
                    state.library.toggleFavourite(track: trackId)
                }
            }

            // Replaces the stock Edit ▸ Undo, which has no undo manager behind
            // it here. Declaring ⌘Z anywhere else just loses to it.
            CommandGroup(replacing: .undoRedo) {
                // ⌘Z while typing is undoing the typing, not the queue — and
                // the field editor has its own undo stack to do it with.
                // On a playlist's page, that playlist's edits; anywhere else,
                // the queue's.
                ShortcutButton(.undo) { state?.player.undo(playlist: state?.nav.openPlaylistId) }
                    .disabledWhileTyping(state?.textFocus)
                ShortcutButton(.redo) { state?.player.redo(playlist: state?.nav.openPlaylistId) }
                    .disabledWhileTyping(state?.textFocus)
            }

            // The queue borrows these, but they must still mean the ordinary
            // thing while typing — ⌘A in the search field selects the text, not
            // the whole queue. EditCommands routes on what has focus.
            CommandGroup(replacing: .pasteboard) {
                ShortcutButton(.cut) {
                    EditCommands.cut { state?.player.cutSelection() }
                }
                ShortcutButton(.copy) {
                    EditCommands.copy { state?.player.copySelection() }
                }
                ShortcutButton(.paste) {
                    EditCommands.paste { state?.player.paste() }
                }
                ShortcutButton(.delete) {
                    EditCommands.delete { state?.player.removeSelected() }
                }
                Divider()
                ShortcutButton(.selectAll) {
                    EditCommands.selectAll { state?.ui.selectAll() }
                }
            }

            // ⌘F means "narrow what I'm looking at" where there is a filter for
            // that, and the library lookup everywhere else.
            CommandGroup(after: .pasteboard) {
                Divider()
                ShortcutButton(.find) {
                    guard let state else { return }
                    if state.nav.section?.filterPlaceholder != nil {
                        state.ui.focusFilter()
                    } else {
                        state.ui.focusSearch()
                    }
                }
            }

            CommandMenu("Queue") {
                Button { Task { await state?.player.saveSession() } } label: {
                    Label("Save Session", koan: Icon.save)
                }
                Button { state?.player.clearQueue() } label: {
                    Label("Clear Queue", koan: Icon.clear)
                }
            }

            CommandGroup(replacing: .help) {
                ShortcutButton(.shortcuts) { state?.ui.showingShortcuts = true }
            }

            CommandMenu("Library") {
                // Each item is disabled only while something holding what it
                // needs is running — a sync does not grey out a rescan. Reads
                // `busy` rather than the task list, which changes on every
                // progress tick and would rebuild the menus with it.
                Group {
                    ShortcutButton(.rescan) { state?.library.scan() }
                    Button { state?.library.scan(force: true) } label: {
                        Label("Force Rescan", koan: Icon.rescanAll)
                    }
                }
                .disabled(state?.activity.conflicts(with: .localLibrary) ?? false)
                Divider()
                Button { state?.library.syncRemote() } label: {
                    Label("Sync", koan: Icon.sync)
                }
                .disabled(state?.activity.conflicts(with: [.remoteTracks]) ?? false)
                Divider()
                Button { state?.art.purge() } label: {
                    Label("Clear Artwork Cache", koan: Icon.clear)
                }
                Button { state?.library.clearDownloads() } label: {
                    Label("Clear Downloaded Files", koan: Icon.clear)
                }
                .disabled(state?.activity.conflicts(with: [.downloads]) ?? false)
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

        // With "Keep running in the menu bar" on, closing the window leaves
        // kōan here, still linked and listening, so other devices can control
        // this Mac.
        MenuBarExtra(
            isInserted: Binding(get: { state?.residency.keepRunning ?? false }, set: { _ in })
        ) {
            if let state { MenuBarMenu(state: state) }
        } label: {
            MenuBarLabel(residency: state?.residency)
        }

        Settings {
            if let state {
                SettingsView()
                    .environment(state)
                    .environment(state.player)
                    .environment(state.library)
                    // A separate scene inherits nothing from the WindowGroup, so
                    // every environment value the settings window reads has to
                    // be listed here — and a missing one is not a compile error,
                    // it is a trap the first time the window opens.
                    .environment(state.activity)
                    .environment(state.art)
                    .environment(state.mirror)
                    // The window has no record to take a colour from: koan's own.
                    .tint(.koanAccent)
                    .environment(\.roomTint, .koanAccent)
                    .koanTheme(state.appearance)
            }
        }
        .defaultSize(width: 820, height: 780)
        .windowResizability(.contentMinSize)
    }
}

/// The menu bar item's icon, and what opens the main window when AppKit asks
/// for it: the item is on screen whenever kōan is resident, and the delegate
/// has no `openWindow` of its own.
private struct MenuBarLabel: View {
    let residency: Residency?
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        KoanIcon(Icon.track)
            .onChange(of: residency?.wantsWindow ?? false, initial: true) { _, wants in
                if wants { residency?.showWindow(with: openWindow) }
            }
    }
}

/// The menu bar item's menu: what is playing, play and pause, next, and the
/// way back to the window.
private struct MenuBarMenu: View {
    let state: AppState
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        let entry = state.mirror.playback.entry
        if let entry {
            Text(entry.title)
            Text(entry.artist)
        } else {
            Text("Nothing playing")
        }
        Divider()
        Button(state.player.isPlaying ? "Pause" : "Play") { state.player.togglePlayPause() }
            .disabled(entry == nil)
        Button("Next") { state.player.next() }
            .disabled(entry == nil)
        Divider()
        Button("Open kōan") { state.residency.showWindow(with: openWindow) }
        Button("Quit kōan") { NSApp.terminate(nil) }
    }
}

/// The library is a file on disk; if it can't be opened there is no app to show.
private struct StartupErrorView: View {
    let message: String

    var body: some View {
        VStack(spacing: 14) {
            KoanIcon(Icon.warning)
                .font(.system(size: 34, weight: .light))
                .foregroundStyle(KoanTheme.style(.bad, system: .orange))
            Text("Couldn't open your library")
                .font(.role(.titleSmall, system: .title3.weight(.medium)))
            Text(message)
                .font(.role(.control, system: .callout))
                .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                .multilineTextAlignment(.center)
                .textSelection(.enabled)

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
