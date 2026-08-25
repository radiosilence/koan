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
                    IOSRootView()
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

/// The phone shell: a tab bar driving the navigator, the transport above it.
struct IOSRootView: View {
    @Environment(Navigator.self) private var nav
    @Environment(PlayerModel.self) private var player

    var body: some View {
        TabView(selection: tab) {
            Tab("Queue", systemImage: "list.bullet", value: TabID.queue) {
                stage { QueueView() }
            }
            Tab("Albums", systemImage: "square.grid.2x2", value: TabID.albums) {
                stage { PageView() }
            }
            Tab("Artists", systemImage: "music.mic", value: TabID.artists) {
                stage { PageView() }
            }
            Tab("Favourites", systemImage: "heart", value: TabID.favourites) {
                stage { PageView() }
            }
            Tab("Settings", systemImage: "gearshape", value: TabID.settings) {
                NavigationStack { SettingsView() }
            }
        }
        // Above the tab bar rather than across the top, and always present —
        // the transport is the one thing that should never be a page you have
        // to navigate back to. `safeAreaInset` would put it *below* the tab bar,
        // which is to say on top of it.
        .tabViewBottomAccessory {
            TransportBar()
                .padding(.horizontal, 8)
        }
        .alert(
            "Something went wrong",
            isPresented: Binding(
                get: { player.lastError != nil },
                set: { if !$0 { player.lastError = nil } }
            ),
            actions: { Button("OK") { player.lastError = nil } },
            message: { Text(player.lastError ?? "") }
        )
    }

    /// A page, with the navigator's own history in front of the tab bar's.
    /// Tapping a record inside a tab goes deeper without leaving it.
    @ViewBuilder private func stage<Content: View>(
        @ViewBuilder _ content: () -> Content
    ) -> some View {
        NavigationStack {
            content()
                .environment(\.onStage, true)
                .toolbar {
                    if nav.canGoBack {
                        ToolbarItem(placement: .topBarLeading) {
                            Button("Back", systemImage: "chevron.left") { nav.goBack() }
                        }
                    }
                }
        }
    }

    private enum TabID: Hashable {
        case queue, albums, artists, favourites, settings
    }

    /// The tab bar and the navigator are the same state seen twice. Selecting a
    /// tab moves the navigator; going deeper inside a tab leaves the selection
    /// where it was, which is what a tab bar is supposed to do.
    private var tab: Binding<TabID> {
        Binding(
            get: {
                switch nav.section {
                case .albums: .albums
                case .artists: .artists
                case .favourites: .favourites
                case .queue, .none: .queue
                default: .queue
                }
            },
            set: { selection in
                switch selection {
                case .queue: nav.show(.queue)
                case .albums: nav.show(.albums)
                case .artists: nav.show(.artists)
                case .favourites: nav.show(.favourites)
                case .settings: break
                }
            }
        )
    }
}
