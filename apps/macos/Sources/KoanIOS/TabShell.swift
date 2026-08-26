import KoanFFI
import SwiftUI

/// The narrow layout: a tab bar, and the transport above it.
///
/// Not a burger menu. A drawer hides the thing koan is mostly about behind a
/// tap, and Apple's own guidance has argued against them for a decade — the
/// answer to "the sidebar does not fit" is a tab bar.
///
/// Reached when there is no room for `RootView`'s sidebar, which is a question
/// about width rather than about which OS this is: an iPad in Slide Over lands
/// here and the same iPad full screen does not. `AdaptiveRootView` decides.
///
/// The navigator stays authoritative either way — the tab bar sets a section,
/// and going deeper inside a tab leaves the selection where it is, which is
/// what a tab bar is for.
struct TabShell: View {
    @Environment(Navigator.self) private var nav
    @Environment(PlayerModel.self) private var player
    @State private var showingNowPlaying = false
    /// Which tab is showing.
    ///
    /// Held rather than derived from the navigator. Opening a record moves the
    /// navigator to a page that is not a section at all, and a binding reading
    /// `nav.section` answers "none" — which read as Queue and threw you out of
    /// the tab you were browsing the moment you tapped an album.
    @State private var selection: TabID = .queue

    var body: some View {
        // The record's colour, behind everything. Not the window's container
        // background the Mac uses — a phone has no window, and the tab bar and
        // the transport float over this rather than sitting beside it.
        ZStack {
            WashLayer()
                .ignoresSafeArea()

            TabView(selection: tab) {
                Tab("Queue", systemImage: Icon.queueSection, value: TabID.queue) {
                    stage { QueueView() }
                }
                Tab("Library", systemImage: "music.note.house", value: TabID.library) {
                    NavigationStack { LibraryTab() }
                }
                Tab("Settings", systemImage: "gearshape", value: TabID.settings) {
                    NavigationStack { SettingsView() }
                }
                Tab(value: TabID.search, role: .search) {
                    NavigationStack { IOSSearchView() }
                }
            }
            .tabViewStyle(.sidebarAdaptable)
            // Above the tab bar rather than below it — `safeAreaInset` would put
            // the transport where the tab bar goes, which is to say on top of it.
            .tabViewBottomAccessory {
                MiniPlayer(showingNowPlaying: $showingNowPlaying)
            }
            // What the app is busy with. The Mac stacks these at the foot of the
            // sidebar; with no sidebar they float above the transport, which is
            // the one part of the screen that is the same wherever you are.
            // Absent when idle, so this is not furniture.
            .overlay(alignment: .bottom) {
                ActivityList()
                    .padding(12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(.regularMaterial, in: .rect(cornerRadius: 16))
                    .padding(.horizontal, 12)
                    // Clear of the mini player and the tab bar under it.
                    .padding(.bottom, 150)
            }
            // Something other than the tab bar can move the navigator —
            // submitting a search, or the queue being asked to show itself.
            .onChange(of: nav.section) { _, section in
                if let owner = Self.tab(for: section), owner != selection {
                    selection = owner
                }
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
    }

    /// A page, with the navigator's own history in front of the tab bar's.
    @ViewBuilder private func stage<Content: View>(
        @ViewBuilder _ content: () -> Content
    ) -> some View {
        NavigationStack {
            content()
                .environment(\.onStage, true)
                .pushesDetailPages()
                .toolbar {
                    if nav.canGoBack {
                        ToolbarItem(placement: .topBarLeading) {
                            Button("Back", systemImage: "chevron.left") { nav.goBack() }
                        }
                    }
                }
        }
    }

    /// Four, deliberately. Five is where iOS starts folding tabs into More,
    /// and More brings a navigation stack of its own.
    enum TabID: Hashable {
        case queue, library, settings, search
    }

    /// The tab bar and the navigator are the same state seen twice.
    ///
    /// Reading the section rather than storing a selection is what keeps them
    /// from disagreeing: opening an album from the Albums tab moves the
    /// navigator to a page that is not a section, and the tab stays where it
    /// was because that is what the deeper page belongs to.
    private var tab: Binding<TabID> {
        Binding(
            get: { selection },
            set: { chosen in
                selection = chosen
                switch chosen {
                case .queue: nav.show(.queue)
                case .search: nav.show(.searchResults)
                // The library tab is a list of sections rather than one of
                // them, and settings is not somewhere the navigator goes.
                case .library, .settings: break
                }
            }
        )
    }

    /// Which tab a section belongs to, or nil for a page that is not a section
    /// — a record or an artist, which belong to whichever tab they were opened
    /// from and must not move the selection.
    private static func tab(for section: Navigator.Section?) -> TabID? {
        switch section {
        case .albums, .artists, .favourites, .playHistory, .playlist: .library
        case .searchResults: .search
        case .queue: .queue
        case .none: nil
        }
    }
}
