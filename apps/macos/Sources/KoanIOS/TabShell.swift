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
/// Where there is room for the sidebar, it is the Mac's: the library's
/// sections and the playlists are tabs of their own, each opening its page
/// beside it, and the Library tab, which only lists them, is hidden. At compact
/// width they fold back behind it.
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
    #if os(tvOS)
    @Environment(AppState.self) private var app
    /// Taken as signed in until the engine says otherwise, so a signed-in TV
    /// never flashes the sign-in page on launch.
    @State private var signedIn = true
    #endif
    @Environment(\.horizontalSizeClass) private var width
    @State private var showingNowPlaying = false
    @State private var showingDevices = false
    /// Which tab is showing. Held rather than derived from the navigator: a
    /// record belongs to whichever tab it was opened from, and the navigator
    /// cannot say which that was.
    #if os(tvOS)
    @State private var selection: TabID = .nowPlaying
    #else
    @State private var selection: TabID = .queue
    #endif

    var body: some View {
        #if os(tvOS)
        // Signed out, the television has nothing to show but the way in: in
        // place of the tabs rather than over them, since Menu dismisses a
        // cover and would leave an empty room behind it.
        Group {
            if signedIn {
                shell
            } else {
                SignInPage { joined() }
            }
        }
        .toggleStyle(SystemSwitch())
        .buttonStyle(TelevisionButton())
        .task { await checkSignedIn() }
        #else
        // The theme's iPad draws its own sidebar beside the tabs, as the Mac's
        // is drawn: the platform's is glass and SF, and folds into a floating
        // capsule of tabs.
        HStack(spacing: 0) {
            if KoanTheme.isOn && sidebar {
                PadSidebar(
                    selection: tab,
                    reselect: reselect,
                    sections: Self.librarySections,
                    play: { play($0, shuffled: $1) }
                )
                .frame(width: 260)
                .koanRule(.trailing)
            }
            shell
        }
        #endif
    }

    private var shell: some View {
        // The record's colour: the tint here, for everything below, and the wash
        // as each tab's navigation background — see `roomBackground()`. A phone
        // has no window to hang one wash on, and a stack paints its own ground
        // over anything placed behind it.
        TabView(selection: tab) {
            #if os(tvOS)
            // The room's first page: what is playing, at the size a sofa reads.
            Tab(Self.title("Now Playing"), systemImage: "play.circle", value: TabID.nowPlaying) {
                NowPlayingPage()
            }
            #endif
            Tab(Self.title("Queue"), systemImage: Icon.queueSection, value: TabID.queue) {
                stack(.queue) { QueueView() }
            }
            #if os(tvOS)
            Tab(Self.title("Library"), systemImage: "music.note.house", value: TabID.library) {
                stack(.library) { LibraryTab() }
            }
            #else
            // Only lists what the sidebar shows as its own rows.
            Tab("Library", systemImage: "music.note.house", value: TabID.library) {
                stack(.library) { LibraryTab() }
            }
            .hidden(sidebar)
            TabSection {
                ForEach(Self.librarySections, id: \.section) { item in
                    Tab(item.title, systemImage: item.icon, value: TabID.section(item.section)) {
                        stack(.section(item.section), grounded: false) { RouteView(route: .page(.section(item.section))) }
                    }
                    // Read only where it can show: each transfer starting or ending
                    // would otherwise re-run the whole shell.
                    .badge(sidebar && item.section == .downloads ? mirror.activeTransfers : 0)
                }
            } header: {
                KoanSectionHeader("Library")
            }
            // With the sidebar folded away, the bar keeps the four tabs; its
            // sidebar button is the way back to these.
            .defaultVisibility(.hidden, for: .tabBar)
            .hidden(!sidebar)
            TabSection {
                ForEach(playlists.playlists, id: \.id) { playlist in
                    Tab(playlist.name, systemImage: Icon.playlist, value: TabID.section(.playlist(playlist.id))) {
                        stack(.section(.playlist(playlist.id)), grounded: false) {
                            RouteView(route: .page(.section(.playlist(playlist.id))))
                        }
                    }
                    .contextMenu {
                        Button("Play", systemImage: Icon.play) { play(playlist) }
                        Button("Shuffle", systemImage: Icon.shuffle) { play(playlist, shuffled: true) }
                    }
                }
            } header: {
                KoanSectionHeader("Playlists")
            }
            // The Mac's "New Playlist…" row.
            .sectionActions {
                Button("New Playlist", systemImage: Icon.add) { playlists.naming = [] }
            }
            .defaultVisibility(.hidden, for: .tabBar)
            .hidden(!sidebar)
            Tab("Settings", systemImage: "gearshape", value: TabID.settings) {
                stack(.settings) { SettingsView() }
            }
            #endif
            #if os(tvOS)
            Tab(Self.title("Search"), systemImage: Icon.search, value: TabID.search, role: .search) {
                stack(.search) { IOSSearchView() }
            }
            #else
            Tab(value: TabID.search, role: .search) {
                stack(.search) { IOSSearchView() }
            }
            #endif
            // Last on a television, where it is visited least.
            #if os(tvOS)
            Tab(Self.title("Settings"), systemImage: "gearshape", value: TabID.settings) {
                stack(.settings) { SettingsView() }
            }
            #endif
        }
        #if os(tvOS)
        // Tabs across the top, as every television app has them; the sidebar
        // style folds them behind a pill a remote has to find first.
        .tabViewStyle(.tabBarOnly)
        #else
        .modifier(AdaptableTabs(sidebar: sidebar))
        .onChange(of: sidebar) { regroup() }
        .onChange(of: playlists.playlists.map(\.id)) { _, ids in dropDeleted(ids) }
        #endif
        .toggleStyle(SystemSwitch())
        .modifier(Transport(
            showingNowPlaying: $showingNowPlaying,
            showingDevices: $showingDevices,
            selection: tab,
            reselect: reselect
        ))
        #if os(tvOS)
        // The remote's Play/Pause, wherever focus is.
        .onPlayPauseCommand { player.togglePlayPause() }
        .shareCodes(player)
        .onChange(of: selection) { Task { await checkSignedIn() } }
        .onChange(of: mirror.connection?.linked) { Task { await checkSignedIn() } }
        .onReceive(NotificationCenter.default.publisher(for: .koanSignedOut)) { _ in
            Task { await checkSignedIn() }
        }
        #endif
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
                    .koanMaterial(.regularMaterial, in: .rect(cornerRadius: KoanTheme.radius(16)))
                    .padding(.horizontal, 12)
                    // Clear of the mini player and the tab bar under it.
                    .padding(.bottom, 150)
                    .transition(.opacity)
            }
        }
        // Something other than the stacks can move the navigator: a link on a
        // page, a search result, Now Playing, playback showing the queue.
        .onChange(of: nav.current) { _, page in arrive(at: page) }
        .modifier(NowPlayingPresentation(isPresented: $showingNowPlaying))
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
        .onChange(of: mirror.connection?.commandNotice?.seq) { _, _ in
            player.show(mirror.connection?.commandNotice)
        }
        .onReceive(NotificationCenter.default.publisher(for: .appResignsActive)) { _ in
            Task { await player.saveOnLeaving() }
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
        #if os(tvOS)
        // Every button that does not choose its own style, sheets and covers
        // included, which take their environment from where they hang; see
        // `TelevisionButton`.
        .buttonStyle(TelevisionButton())
        #endif
    }

    /// A tab's navigation stack. Pages are drawn from their routes — see
    /// `RouteView` — and the navigator follows whatever is on top.
    /// `grounded: false` for a root that is a `RouteView`, which grounds itself.
    private func stack<Root: View>(
        _ tab: TabID, grounded: Bool = true, @ViewBuilder root: () -> Root
    ) -> some View {
        let routes = paths[tab] ?? []
        // On stage is the top of the tab in front, and nothing else. A stack
        // keeps every page it pushed and a tab view keeps every tab, and a
        // playing indicator on any of them would keep the analyser running for
        // bars nobody can see.
        let showing = tab == selection
        return NavigationStack(path: path(tab)) {
            Group {
                if grounded {
                    root().washedGround().roomBackground()
                } else {
                    root()
                }
            }
            .koanHidesSystemTabBar()
            .environment(\.onStage, showing && routes.isEmpty)
            .navigationDestination(for: Route.self) { route in
                RouteView(route: route)
                    .koanBackButton()
                    .koanHidesSystemTabBar()
                    .environment(\.onStage, showing && route == routes.last)
            }
        }
        .id(resets[tab, default: 0])
    }

    /// Four in the tab bar, deliberately. Five is where iOS starts folding tabs
    /// into More, and More brings a navigation stack of its own.
    enum TabID: Hashable {
        case queue, library, settings, search
        /// tvOS only, where Now Playing is a page rather than a sheet.
        case nowPlaying
        /// A row of the iPad's sidebar: a library section or a playlist.
        case section(Navigator.Section)

        /// The page the tab itself is, under anything pushed onto it. The
        /// library is a list of sections rather than one, and settings is not
        /// somewhere the navigator goes.
        var root: Navigator.Page? {
            switch self {
            case .queue: .section(.queue)
            case .search: .section(.searchResults)
            case .section(let section): .section(section)
            case .library, .settings, .nowPlaying: nil
            }
        }
    }

    /// What each tab has pushed. Held per tab, so leaving one and coming back
    /// finds it where it was.
    @State private var paths: [TabID: [Route]] = [:]

    /// How many times each tab's stack has been made again — see `reselect`.
    @State private var resets: [TabID: Int] = [:]

    /// The tab showing, chosen again: back to its root, and at the root, back
    /// to the top, as a tab bar does. Settings pushes its panes by link rather
    /// than by route, so its path can be empty with a pane on screen: a stack
    /// with nothing in its path is made again, which pops what it holds and
    /// starts its page at the top.
    private func reselect(_ tab: TabID) {
        if (paths[tab] ?? []).isEmpty {
            resets[tab, default: 0] += 1
        } else {
            paths[tab] = []
        }
    }

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
        // First: a pop back to a page another tab owns stays in this tab.
        guard top(of: selection) != page else { return }
        if let owner = owner(of: page) {
            paths[owner] = []
            selection = owner
            return
        }
        var routes = paths[selection] ?? []
        if let index = routes.lastIndex(of: .page(page)) {
            routes.removeSubrange((index + 1)...)
        } else {
            routes.append(.page(page))
        }
        paths[selection] = routes
    }

    /// The tab whose own page this is, if one is showing.
    private func owner(of page: Navigator.Page) -> TabID? {
        if let tab = [TabID.queue, .search].first(where: { $0.root == page }) { return tab }
        guard sidebar, let section = page.section else { return nil }
        let listed = if case .playlist(let id) = section {
            playlists.playlist(id: id) != nil
        } else {
            Self.librarySections.contains { $0.section == section }
        }
        return listed ? .section(section) : nil
    }

    /// The width changed across the line where the sidebar appears. A sidebar
    /// tab folds into the Library tab's stack, under the row that leads to it,
    /// and back out again the other way.
    private func regroup() {
        if sidebar {
            guard selection == .library else { return }
            var routes = paths[.library] ?? []
            paths[.library] = []
            if routes.first == .playlists { routes.removeFirst() }
            var owner = TabID.section(.albums)
            if let page = routes.first?.page, let tab = self.owner(of: page) {
                owner = tab
                routes.removeFirst()
            }
            paths[owner] = routes
            selection = owner
            follow(owner)
        } else if case .section(let section) = selection {
            let lead: [Route] = if case .playlist = section { [.playlists] } else { [] }
            paths[.library] = lead + [.page(.section(section))] + (paths[selection] ?? [])
            paths[selection] = []
            selection = .library
        }
    }

    /// The playlist showing in the sidebar was deleted elsewhere — on the
    /// server, another device, or with its file. Its tab has gone, so leave it
    /// for the queue, as the Mac does on deleting one.
    private func dropDeleted(_ ids: [Int64]) {
        guard case .section(.playlist(let id)) = selection, !ids.contains(id) else { return }
        paths[selection] = nil
        nav.forget(.playlist(id))
        nav.show(.queue)
    }

    /// A tab's title: lowercase in the theme on a television, whose tab bar
    /// is the one the theme keeps.
    private static func title(_ text: String) -> String {
        #if os(tvOS)
        KoanTheme.label(text)
        #else
        text
        #endif
    }

    /// Whether the sidebar is the navigation: an iPad with room for it.
    private var sidebar: Bool {
        #if os(tvOS)
        false
        #else
        width == .regular
        #endif
    }

    /// The sidebar's library rows, in the Mac's order. No Tracks, for the
    /// reason the Library tab gives.
    private static let librarySections: [(section: Navigator.Section, title: String, icon: String)] = [
        (.albums, "Albums", Icon.album),
        (.artists, "Artists", Icon.artist),
        (.favourites, "Favourites", Icon.favourite),
        (.recentlyPlayed, "Recently Played", Icon.recentlyPlayed),
        (.onDevice, "Downloaded", Icon.onDevice),
        (.playHistory, "History", Icon.history),
        (.downloads, "Downloads", Icon.downloads),
    ]

    /// Play it where you stand, as the Mac's sidebar does.
    private func play(_ playlist: Playlist, shuffled: Bool = false) {
        let engine = playlists.engine
        Task {
            _ = try? await engine.playPlaylist(
                playlistId: playlist.id, startEntry: nil, shuffled: shuffled
            )
        }
    }

    #if os(tvOS)
    private func checkSignedIn() async {
        signedIn = await app.engine.settings().remoteSignedIn
    }

    /// Signed in by pairing or the account form: load the library, as joining
    /// with an invite does.
    private func joined() {
        signedIn = true
        let engine = app.engine
        Task {
            let synced = await activity.run(
                "Loading the library", uses: [.remoteTracks], followsSync: true
            ) {
                try await engine.syncRemote()
            }
            if case .failure(let error) = synced {
                player.lastError = SettingsModel.describe(error)
            }
        }
    }
    #endif

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

/// The mini player. On a phone it sits above the tab bar: `safeAreaInset`
/// would put it where the tab bar goes, which is to say on top of it.
private struct Transport: ViewModifier {
    @Binding var showingNowPlaying: Bool
    @Binding var showingDevices: Bool
    @Binding var selection: TabShell.TabID
    /// The tab already showing, chosen again: back to its root, as a tab bar does.
    let reselect: (TabShell.TabID) -> Void
    @Environment(\.horizontalSizeClass) private var width
    /// The bar's height as laid out, which Dynamic Type moves.
    @State private var barHeight: CGFloat = 0
    /// How far the keyboard reaches up from the foot of the screen: zero while
    /// none is up, and for one floating clear of the foot. The bar is behind a
    /// docked one, and the room a page keeps for the bar would sit above it as
    /// a blank strip over the page's own controls.
    @State private var keyboardOverlap: CGFloat = 0
    @Namespace private var underline

    func body(content: Content) -> some View {
        #if os(tvOS)
        // Now Playing is a tab of its own there.
        content
        #else
        if KoanTheme.isOn {
            // The theme's own bar in place of the platform's glass: the mini
            // player as a row with the playhead along its top, and on a phone
            // the tabs flat beneath it; an iPad's are its sidebar. Laid over the content and kept behind the keyboard,
            // as the platform's tab bar is. Each page makes room for it itself,
            // from its height (`koanHidesSystemTabBar`), less what the keyboard
            // already covers.
            content
                .environment(\.koanBarHeight, max(0, barHeight - keyboardOverlap))
                // Posted on showing, hiding and every change of size between,
                // in the coordinates of the screen it is the object of.
                .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardWillChangeFrameNotification)) {
                    guard let frame = $0.userInfo?[UIResponder.keyboardFrameEndUserInfoKey] as? CGRect else { return }
                    let foot = ($0.object as? UIScreen)?.bounds.maxY ?? frame.maxY
                    keyboardOverlap = max(0, foot - frame.minY)
                }
                .overlay(alignment: .bottom) {
                    VStack(spacing: 0) {
                        player
                            .padding(.vertical, 8)
                            .overlay(alignment: .top) { MiniPlayhead() }
                            .koanRule(.top)
                        if width == .compact {
                            tabs
                                .koanRule(.top)
                        }
                    }
                    .koanSurface()
                    // What a test measures a page's last row against.
                    .accessibilityElement(children: .contain)
                    .accessibilityIdentifier("koan-bar")
                    .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { barHeight = $0 }
                    .ignoresSafeArea(.keyboard, edges: .bottom)
                }
        } else {
            content.tabViewBottomAccessory { player }
        }
        #endif
    }

    #if !os(tvOS)
    private var tabs: some View {
        HStack(spacing: 0) {
            ForEach(Array(Self.items.enumerated()), id: \.element.id) { index, item in
                Button {
                    if selection == item.id { reselect(item.id) } else { selection = item.id }
                } label: {
                    KoanTabItem(
                        title: item.title, icon: item.icon, selected: selection == item.id,
                        underline: underline, position: (index, Self.items.count)
                    )
                }
                .buttonStyle(.plain)
            }
        }
        // The platform's bar's height, the items centred in it as its are.
        // Below it, the home indicator's strip takes the bar's ground and
        // nothing else.
        .frame(minHeight: 49)
        // Capped, as the platform's bar is, with the large content viewer
        // past the cap.
        .dynamicTypeSize(...DynamicTypeSize.xxxLarge)
        // The underline slides to the tab chosen; the page itself swaps at once.
        .koanAnimation(KoanTheme.Motion.normal, value: selection)
    }

    private static let items: [(id: TabShell.TabID, title: String, icon: String)] = [
        (.queue, "Queue", Icon.queueSection),
        (.library, "Library", "music.note.house"),
        (.settings, "Settings", "gearshape"),
        (.search, "Search", Icon.search),
    ]
    #endif

    private var player: some View {
        MiniPlayer(showingNowPlaying: $showingNowPlaying, showingDevices: $showingDevices)
    }
}

#if !os(tvOS)
/// The playhead along the top of the theme's mini player: two points of the
/// accent, handed to the render server as the seek bar's is.
private struct MiniPlayhead: View {
    @Environment(PlayerModel.self) private var player

    var body: some View {
        SeekProgress(fraction: player.progress, remaining: runway, thickness: 2, showsHead: false)
            .frame(height: 2)
            .allowsHitTesting(false)
    }

    private var runway: TimeInterval {
        guard player.scrubbing == nil, player.playhead.playing, player.durationMs > 0 else { return 0 }
        return Double(player.durationMs - player.playhead.at(within: player.durationMs)) / 1000
    }
}
#endif

#if !os(tvOS)
/// The tab view's layout. In the platform's look, its own iPad layout: a bar
/// across the top that opens into a sidebar. In the theme, tabs alone, with
/// the bar hidden on every page (`koanHidesSystemTabBar`) for the theme's bar
/// on a phone and `PadSidebar` on an iPad.
private struct AdaptableTabs: ViewModifier {
    let sidebar: Bool
    @Environment(EngineMirror.self) private var mirror

    func body(content: Content) -> some View {
        if KoanTheme.isOn {
            content.tabViewStyle(.tabBarOnly)
        } else {
            content
                .tabViewStyle(.sidebarAdaptable)
                // Open, as the Mac's is: it is where everything but the queue lives.
                .defaultAdaptableTabBarPlacement(.sidebar)
                // What the Library tab says atop its list, with no Library tab to say it.
                .tabViewSidebarHeader {
                    if sidebar, LibraryStatus.showing(mirror) {
                        VStack(alignment: .leading, spacing: 6) { LibraryStatus() }
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
        }
    }
}

/// The theme's iPad sidebar: the Mac's navigation rows, flat on the ground.
/// The same tabs as the platform's sidebar, chosen the same way.
private struct PadSidebar: View {
    @Binding var selection: TabShell.TabID
    let reselect: (TabShell.TabID) -> Void
    let sections: [(section: Navigator.Section, title: String, icon: String)]
    let play: (Playlist, _ shuffled: Bool) -> Void
    @Environment(EngineMirror.self) private var mirror
    @Environment(PlaylistsModel.self) private var playlists

    var body: some View {
        List {
            if LibraryStatus.showing(mirror) {
                VStack(alignment: .leading, spacing: 6) { LibraryStatus() }
                    .font(.koan(.meta))
                    .listRowBackground(Color.clear)
            }
            Section {
                row(.queue, "Queue", Icon.queueSection)
                row(.search, "Search", Icon.search)
                row(.settings, "Settings", "gearshape")
            }
            Section {
                ForEach(sections, id: \.section) { item in
                    row(.section(item.section), item.title, item.icon)
                        .badge(item.section == .downloads ? mirror.activeTransfers : 0)
                        .listRowSeparator(.hidden)
                }
            } header: {
                KoanSectionHeader("Library")
            }
            Section {
                ForEach(playlists.playlists, id: \.id) { playlist in
                    row(.section(.playlist(playlist.id)), playlist.name, Icon.playlist, data: true)
                        .contextMenu {
                            Button("Play", systemImage: Icon.play) { play(playlist, false) }
                            Button("Shuffle", systemImage: Icon.shuffle) { play(playlist, true) }
                        }
                        .listRowSeparator(.hidden)
                }
                Button { playlists.naming = [] } label: {
                    KoanLabel("New Playlist", icon: Icon.add)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .koanNavRow(selected: false)
                .listRowSeparator(.hidden)
            } header: {
                KoanSectionHeader("Playlists")
            }
        }
        .listStyle(.plain)
        .koanSidebar()
        .environment(\.defaultMinListHeaderHeight, 0)
        .listRowSeparator(.hidden)
        .listSectionSeparator(.hidden)
    }

    /// A row that is a tab: chosen again, back to its root, as a tab is.
    /// A playlist's name is the person's, and keeps its case.
    private func row(_ id: TabShell.TabID, _ title: String, _ icon: String, data: Bool = false) -> some View {
        Button {
            if selection == id { reselect(id) } else { selection = id }
        } label: {
            Label {
                Text(title).textCase(data ? nil : .lowercase)
            } icon: {
                KoanIcon(icon)
            }
            .labelStyle(PadRowLabel())
            .lineLimit(1)
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .koanNavRow(selected: selection == id)
        .listRowSeparator(.hidden)
    }
}

/// A row's glyph, or not, as "Show icons" says.
private struct PadRowLabel: LabelStyle {
    @Environment(\.koanIcons) private var icons

    func makeBody(configuration: Configuration) -> some View {
        if icons {
            Label(configuration).labelStyle(.titleAndIcon)
        } else {
            Label(configuration).labelStyle(.titleOnly)
        }
    }
}
#endif
