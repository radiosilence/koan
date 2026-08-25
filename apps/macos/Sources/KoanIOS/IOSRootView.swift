import KoanFFI
import SwiftUI

/// The phone shell.
///
/// Not a burger menu. A drawer hides the thing koan is mostly about behind a
/// tap, and Apple's own guidance has argued against them for a decade — the
/// iOS answer to "the sidebar does not fit" is a tab bar.
///
/// `.sidebarAdaptable` is what makes it one piece of code: an iPhone gets the
/// tab bar, an iPad gets the sidebar the Mac has, and the tabs beyond the fifth
/// collapse into More rather than being dropped. The navigator stays
/// authoritative either way — the tab bar sets a section, and going deeper
/// inside a tab leaves the selection where it is, which is what a tab bar is
/// for.
struct IOSRootView: View {
    @Environment(Navigator.self) private var nav
    @Environment(PlayerModel.self) private var player
    @State private var showingNowPlaying = false

    var body: some View {
        TabView(selection: tab) {
            Tab("Queue", systemImage: Icon.queueSection, value: TabID.queue) {
                stage { QueueView() }
            }
            Tab("Albums", systemImage: Icon.album, value: TabID.albums) {
                stage { PageView() }
            }
            Tab("Artists", systemImage: Icon.artist, value: TabID.artists) {
                stage { PageView() }
            }
            Tab("Favourites", systemImage: Icon.favourite, value: TabID.favourites) {
                stage { PageView() }
            }
            Tab("Playlists", systemImage: Icon.playlist, value: TabID.playlists) {
                NavigationStack { PlaylistsList() }
            }
            Tab("History", systemImage: Icon.history, value: TabID.history) {
                stage { PageView() }
            }
            Tab("Settings", systemImage: "gearshape", value: TabID.settings) {
                NavigationStack { SettingsView() }
            }
            Tab(value: TabID.search, role: .search) {
                NavigationStack { IOSSearchView() }
            }
        }
        .tabViewStyle(.sidebarAdaptable)
        // Above the tab bar rather than below it — `safeAreaInset` would put the
        // transport where the tab bar goes, which is to say on top of it.
        .tabViewBottomAccessory {
            MiniPlayer(showingNowPlaying: $showingNowPlaying)
        }
        .sheet(isPresented: $showingNowPlaying) {
            NowPlayingSheet()
                .presentationDetents([.large])
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

    enum TabID: Hashable {
        case queue, albums, artists, favourites, playlists, history, settings, search
    }

    /// The tab bar and the navigator are the same state seen twice.
    ///
    /// Reading the section rather than storing a selection is what keeps them
    /// from disagreeing: opening an album from the Albums tab moves the
    /// navigator to a page that is not a section, and the tab stays where it
    /// was because that is what the deeper page belongs to.
    private var tab: Binding<TabID> {
        Binding(
            get: {
                switch nav.section {
                case .albums: .albums
                case .artists: .artists
                case .favourites: .favourites
                case .playHistory: .history
                case .playlist: .playlists
                case .searchResults: .search
                case .queue, .none: .queue
                }
            },
            set: { selection in
                switch selection {
                case .queue: nav.show(.queue)
                case .albums: nav.show(.albums)
                case .artists: nav.show(.artists)
                case .favourites: nav.show(.favourites)
                case .history: nav.show(.playHistory)
                case .search: nav.show(.searchResults)
                // Neither is a section: playlists is a list of them, and
                // settings is not somewhere the navigator goes at all.
                case .playlists, .settings: break
                }
            }
        )
    }
}
