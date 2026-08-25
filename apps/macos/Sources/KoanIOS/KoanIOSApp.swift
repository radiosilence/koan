import KoanFFI
import SwiftUI

/// koan on iOS.
///
/// The same engine, the same models and the same pages as the Mac app — what
/// differs is the shell around them. A phone has no menu bar, no sidebar and no
/// second window, so the navigator is driven by a tab bar and the transport
/// sits above it rather than across the top.
@main
struct KoanIOSApp: App {
    @State private var state: AppState?
    @State private var startupError: String?
    @State private var session = AudioSession()

    var body: some Scene {
        WindowGroup {
            Group {
                if let state {
                    AdaptiveRootView()
                        .environment(state)
                        .environment(state.player)
                        .environment(state.library)
                        .environment(state.nav)
                        .environment(state.search)
                        .environment(state.art)
                        .environment(state.organize)
                        .environment(state.playlists)
                        .environment(state.activity)
                        .environment(state.levels)
                        .environment(state.ui)
                        .tint(.koanAccent)
                } else if let startupError {
                    ContentUnavailableView(
                        "koan could not start",
                        systemImage: "exclamationmark.triangle",
                        description: Text(startupError)
                    )
                } else {
                    ProgressView().controlSize(.large)
                }
            }
            .task {
                guard state == nil, startupError == nil else { return }
                do {
                    let built = try await AppState()
                    await built.start()
                    // The session goes up before anything can be asked to play:
                    // a RemoteIO unit on an inactive session produces silence
                    // and reports success, which is the worst of both.
                    session.activate()
                    session.onInterrupted = { [weak built] in built?.player.pause() }
                    session.onRouteLost = { [weak built] in built?.player.pause() }
                    // Deliberately not resuming on `onResumable`: coming back
                    // from a call should not start the music in your pocket.
                    state = built
                } catch {
                    startupError = String(describing: error)
                }
            }
        }
    }
}
