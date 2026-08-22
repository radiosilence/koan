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
    let art: CoverArtCache

    init() throws {
        let engine = try KoanEngine()
        self.engine = engine
        self.player = PlayerModel(engine: engine)
        self.library = LibraryModel(engine: engine)
        self.art = CoverArtCache(engine: engine)
    }
}

@main
struct KoanApp: App {
    @State private var state: AppState?
    @State private var startupError: String?
    @State private var showingPicker = false

    var body: some Scene {
        Window("koan", id: "main") {
            Group {
                if let state {
                    RootView(showingPicker: $showingPicker)
                        .environment(state)
                        .environment(state.player)
                        .environment(state.library)
                        .environment(state.art)
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
                    created.player.startPolling()
                    created.library.loadInitial()
                    state = created
                } catch {
                    startupError = String(describing: error)
                }
            }
        }
        .windowToolbarStyle(.unified(showsTitle: false))
        .commands {
            CommandGroup(after: .newItem) {
                Button("Add Music…") { showingPicker = true }
                    .keyboardShortcut("k", modifiers: .command)
            }
            CommandMenu("Playback") {
                Button(state?.player.isPlaying == true ? "Pause" : "Play") {
                    state?.player.togglePlayPause()
                }
                .keyboardShortcut(.space, modifiers: [])
                Button("Next") { state?.player.next() }
                    .keyboardShortcut(.rightArrow, modifiers: .command)
                Button("Previous") { state?.player.previous() }
                    .keyboardShortcut(.leftArrow, modifiers: .command)
                Divider()
                Button("Clear Queue") { state?.player.clearQueue() }
                Button("Undo") { state?.player.undo() }
                    .keyboardShortcut("z", modifiers: .command)
                Button("Redo") { state?.player.redo() }
                    .keyboardShortcut("z", modifiers: [.command, .shift])
            }
            CommandGroup(after: .toolbar) {
                Button("Rescan Library") { state?.library.scan() }
                    .keyboardShortcut("r", modifiers: [.command, .shift])
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
