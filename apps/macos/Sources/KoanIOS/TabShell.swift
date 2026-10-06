import KoanFFI
import SwiftUI

/// The iOS layout: a tab bar, and the transport above it.
///
/// A tab bar rather than a drawer, which would hide the thing koan is mostly
/// about behind a tap.
///
/// Every iPhone and iPad. `sidebarAdaptable` makes the tab bar the platform's
/// own iPad layout, a bar across the top that opens into a sidebar, and it is
/// one shell to test rather than two. `RootView`'s split view is the Mac's.
///
/// The navigator stays authoritative either way — the tab bar sets a section,
/// and going deeper inside a tab leaves the selection where it is, which is
/// what a tab bar is for.
struct TabShell: View {
    @Environment(Navigator.self) private var nav
    @Environment(PlayerModel.self) private var player
    @Environment(CoverArtCache.self) private var art
    @Environment(LibraryModel.self) private var library
    @Environment(EngineMirror.self) private var mirror
    @Environment(PlaylistsModel.self) private var playlists
    @Environment(ActivityModel.self) private var activity
    @State private var showingNowPlaying = false
    @State private var showingDevices = false
    /// Which tab is showing. Held rather than derived from the navigator: a
    /// record belongs to whichever tab it was opened from, and the navigator
    /// cannot say which that was.
    @State private var selection: TabID = .queue

    var body: some View {
        // The record's colour: the tint here, for everything below, and the wash
        // as each tab's navigation background — see `roomBackground()`. A phone
        // has no window to hang one wash on, and a stack paints its own ground
        // over anything placed behind it.
        TabView(selection: tab) {
            Tab("Queue", systemImage: Icon.queueSection, value: TabID.queue) {
                stack(.queue) { QueueView() }
            }
            Tab("Library", systemImage: "music.note.house", value: TabID.library) {
                stack(.library) { LibraryTab() }
            }
            Tab("Settings", systemImage: "gearshape", value: TabID.settings) {
                stack(.settings) { SettingsView() }
            }
            Tab(value: TabID.search, role: .search) {
                stack(.search) { IOSSearchView() }
            }
        }
        .tabViewStyle(.sidebarAdaptable)
        .toggleStyle(SystemSwitch())
        // Above the tab bar rather than below it — `safeAreaInset` would put
        // the transport where the tab bar goes, which is to say on top of it.
        .tabViewBottomAccessory {
            MiniPlayer(showingNowPlaying: $showingNowPlaying, showingDevices: $showingDevices)
        }
        .controlSheet(isPresented: $showingDevices)
        // What the app is busy with. The Mac stacks these at the foot of the
        // sidebar; with no sidebar they float above the transport, which is
        // the one part of the screen that is the same wherever you are.
        // Absent when idle, so this is not furniture.
        .overlay(alignment: .bottom) {
            // The card, not only its rows: an empty list inside a material
            // still draws the material.
            if !activity.tasks.isEmpty {
                ActivityList()
                    .padding(12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(.regularMaterial, in: .rect(cornerRadius: 16))
                    .padding(.horizontal, 12)
                    // Clear of the mini player and the tab bar under it.
                    .padding(.bottom, 150)
                    .transition(.opacity)
            }
        }
        // Something other than the stacks can move the navigator: a link on a
        // page, a search result, Now Playing, playback showing the queue.
        .onChange(of: nav.current) { _, page in arrive(at: page) }
        .sheet(isPresented: $showingNowPlaying) {
            NowPlayingSheet()
                .presentationDetents([.large])
        }
        .modifier(RecordRoom())
        // As on the Mac: the things that ask for a new playlist are mostly
        // context menus, which take their own alerts down with them.
        .newPlaylistAlert()
        // What `RootView` does for the wide layout: the one place a library
        // change reaches the app's own lists, and the last dependable moment
        // to save the queue before iOS suspends the app.
        .reloading(on: 0) {
            // First, so nothing redrawn below picks up a cover cached under an
            // id the library has since given to another record.
            await art.applyEvictions()
            library.libraryChanged()
            playlists.load()
        }
        // A play recorded, or plays forgotten: the pages derived from
        // history ask again.
        .onChange(of: mirror.historyVersion) { _, _ in library.historyChanged() }
        // Offline narrows every listing to what can play here; going online
        // widens it again.
        .onChange(of: mirror.connection?.offline ?? false) { _, _ in library.libraryChanged() }
        .onReceive(NotificationCenter.default.publisher(for: .appResignsActive)) { _ in
            Task { await player.saveSession() }
        }
        // One alert for both, as the Mac has one toast slot: a failure
        // outranks a notice, and only a failure is titled as one.
        .alert(
            player.lastError != nil ? "Something went wrong" : (player.lastNotice ?? ""),
            isPresented: Binding(
                get: { player.lastError != nil || player.lastNotice != nil },
                set: {
                    if !$0 {
                        player.lastError = nil
                        player.lastNotice = nil
                    }
                }
            ),
            actions: {
                Button("OK") {
                    player.lastError = nil
                    player.lastNotice = nil
                }
            },
            message: { Text(player.lastError ?? "") }
        )
    }

    /// A tab's navigation stack. Pages are drawn from their routes — see
    /// `RouteView` — and the navigator follows whatever is on top.
    private func stack<Root: View>(
        _ tab: TabID, @ViewBuilder root: () -> Root
    ) -> some View {
        let routes = paths[tab] ?? []
        // On stage is the top of the tab in front, and nothing else. A stack
        // keeps every page it pushed and a tab view keeps every tab, and a
        // playing indicator on any of them would keep the analyser running for
        // bars nobody can see.
        let showing = tab == selection
        return NavigationStack(path: path(tab)) {
            root()
                .environment(\.onStage, showing && routes.isEmpty)
                .washedGround()
                .roomBackground()
                .navigationDestination(for: Route.self) { route in
                    RouteView(route: route)
                        .environment(\.onStage, showing && route == routes.last)
                }
        }
    }

    /// Four, deliberately. Five is where iOS starts folding tabs into More,
    /// and More brings a navigation stack of its own.
    enum TabID: Hashable {
        case queue, library, settings, search

        /// The page the tab itself is, under anything pushed onto it. The
        /// library is a list of sections rather than one, and settings is not
        /// somewhere the navigator goes.
        var root: Navigator.Page? {
            switch self {
            case .queue: .section(.queue)
            case .search: .section(.searchResults)
            case .library, .settings: nil
            }
        }
    }

    /// What each tab has pushed. Held per tab, so leaving one and coming back
    /// finds it where it was.
    @State private var paths: [TabID: [Route]] = [:]

    private func path(_ tab: TabID) -> Binding<[Route]> {
        Binding(
            get: { paths[tab] ?? [] },
            set: { routes in
                paths[tab] = routes
                follow(tab)
            }
        )
    }

    /// The page on top of a tab: the last navigator page pushed, else the tab's
    /// own.
    private func top(of tab: TabID) -> Navigator.Page? {
        (paths[tab] ?? []).reversed().lazy.compactMap(\.page).first ?? tab.root
    }

    /// Bring the navigator to what the stack now shows — after a push, a pop, a
    /// swipe back, or a change of tab. The library's listings load by moving
    /// it, so a page that did not move it would draw the last one's rows.
    private func follow(_ tab: TabID) {
        guard tab == selection, let page = top(of: tab), page != nav.current else { return }
        nav.go(to: page)
    }

    /// The navigator moved on its own account; show where it went. A tab's own
    /// page brings that tab forward, back at its root. Anything else is pushed
    /// on the tab in front, or popped back to if it is already in the stack.
    private func arrive(at page: Navigator.Page) {
        if let owner = [TabID.queue, .search].first(where: { $0.root == page }) {
            paths[owner] = []
            selection = owner
            return
        }
        guard top(of: selection) != page else { return }
        var routes = paths[selection] ?? []
        if let index = routes.lastIndex(of: .page(page)) {
            routes.removeSubrange((index + 1)...)
        } else {
            routes.append(.page(page))
        }
        paths[selection] = routes
    }

    private var tab: Binding<TabID> {
        Binding(
            get: { selection },
            set: { chosen in
                selection = chosen
                follow(chosen)
            }
        )
    }
}
