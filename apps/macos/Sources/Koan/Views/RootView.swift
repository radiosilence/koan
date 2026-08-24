import KoanFFI
import SwiftUI

/// Sidebar, stage, optional lyrics inspector, transport pinned to the bottom.
///
/// The stage defaults to the queue rather than the library — koan is a player
/// you build a queue in, and the TUI opens the same way. The library is
/// somewhere you go to feed it.
///
/// `NavigationSplitView` is the root and stays the root. Wrapping it in a stack
/// or putting an `HSplitView` in its detail column breaks width propagation:
/// `HSplitView` sizes children to their minimum, so the stage would sit at
/// whatever `minWidth` it declared no matter how large the window got, and an
/// adaptive grid inside it would be stuck at two columns. The transport is a
/// `safeAreaInset` and the lyrics panel an `inspector` for the same reason —
/// both add chrome without taking the detail column's width away.
///
/// There is one `NavigationStack` for the whole detail column and one owner of
/// its path, `Navigator`. Per-browser stacks can't be driven from outside, and
/// search needs to push you into one.
struct RootView: View {
    let hotkeys: Hotkeys

    @Environment(UIState.self) private var ui
    @Environment(LibraryModel.self) private var library
    @Environment(Navigator.self) private var nav
    @Environment(SearchModel.self) private var search
    @Environment(PlayerModel.self) private var player
    /// Read here only to hand back to the window background — see below.
    @Environment(CoverArtCache.self) private var art

    @AppStorage("showLyrics") private var showLyrics = false
    @State private var transportHeight: CGFloat = 0
    /// The record the window is washed in.
    ///
    /// Held rather than derived from `currentTrackId` on every frame: artwork
    /// is fetched per *track*, so re-deriving it would ask the server for a new
    /// copy of the same sleeve at every track change within a record. It only
    /// moves when the record does.
    @State private var bleed: AlbumArtwork.Source?
    /// The colour of the record playing. The app's own accent is a neutral, so
    /// the only colour in the chrome is the one the music brought.
    @State private var recordTint: Color?
    /// Watched rather than inferred from the measured width: a collapsed
    /// sidebar still reports its last width, so the transport kept a gap where
    /// it used to be.
    @State private var columns: NavigationSplitViewVisibility = .automatic

    /// What the window is washed in: the record you opened, or the one playing,
    /// and only where either means something. A library grid is its own colour.
    private var washSource: AlbumArtwork.Source? {
        if case .album(let id) = nav.stack.wrappedValue.last {
            .album(id)
        } else if nav.section == .queue {
            bleed
        } else {
            nil
        }
    }

    /// What the *controls* take their colour from. The page you are on first —
    /// an album's own record — and what is playing everywhere else, so a
    /// favourites list is still coloured by the music rather than by nothing.
    private var tintSource: AlbumArtwork.Source? {
        if case .album(let id) = nav.stack.wrappedValue.last {
            .album(id)
        } else {
            bleed
        }
    }

    /// Which record is playing — artist as well as title, because "Greatest
    /// Hits" is not one record.
    private var playingRecord: String? {
        player.currentEntry.map { "\($0.albumArtist)\u{1}\($0.album)" }
    }

    var body: some View {
        @Bindable var library = library
        @Bindable var search = search
        @Bindable var ui = ui

        // The window background is evaluated by the *scene*, outside every
        // environment `RootView` was handed, so anything it needs is captured
        // here. Reading an `@Environment` inside that closure — including to
        // put one back — traps, and the app dies on launch.
        let wash = washSource
        let washDrifts = player.isPlaying
        let artCache = art

        NavigationSplitView(columnVisibility: $columns) {
            SidebarView()
                .navigationSplitViewColumnWidth(min: 190, ideal: 215, max: 290)
        } detail: {
            NavigationStack(path: nav.stack) {
                StageView()
                    .clearsTransport(transportHeight)
                    // A page with no wash needs a ground of its own. The
                    // window's is the wash now, so anything transparent
                    // composites onto it — and onto the page you came from,
                    // which is what a library grid was doing.
                    .background(washSource == nil ? AnyShapeStyle(.background) : AnyShapeStyle(.clear))
                    // The stack draws its own back button for pushed
                    // destinations, next to the pair we already have — three
                    // chevrons in a row. Ours can cross sections and search
                    // jumps, which the stack's cannot, so the stack's goes.
                    .navigationDestination(for: Route.self) { route in
                        switch route {
                        case .album(let id):
                            AlbumDetailView(albumId: id)
                                .navigationBarBackButtonHidden(true)
                                .clearsTransport(transportHeight)
                        case .artist(let id):
                            ArtistDetailView(artistId: id)
                                .navigationBarBackButtonHidden(true)
                                .clearsTransport(transportHeight)
                                // An artist is a shelf of records rather than
                                // one, so there is no wash to show through.
                                .background(.background)
                        }
                    }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .inspector(isPresented: $showLyrics) {
            LyricsPanel()
                .inspectorColumnWidth(min: 260, ideal: 320, max: 460)
                // The toggle belongs to the inspector rather than the window, so
                // it sits at the pane's leading edge and moves with it. In the
                // window's trailing group the pane opened out from underneath
                // it, and it shared a capsule with the filter field.
                .toolbar {
                    ToolbarItem(placement: .primaryAction) {
                        Button {
                            showLyrics.toggle()
                        } label: {
                            Label("Lyrics", systemImage: "quote.bubble")
                        }
                        .help("Lyrics panel (⌥⌘L)")
                    }
                }
        }
        // The queue is a list of names, and the record playing is the only
        // thing in it with a colour. On the *window* rather than behind the
        // queue: nothing inside a split view column reaches past the toolbar's
        // inset, and a wash that stops in a line under the toolbar is worse
        // than none. An album page washes its own header, so the window stays
        // out of its way.
        .containerBackground(for: .window) {
            // Over an opaque ground, because this *replaces* the window's own
            // background rather than sitting on it — a half-transparent wash on
            // its own leaves you looking through the app at the desktop.
            ZStack {
                Rectangle().fill(.background)
                ArtworkBleed(source: wash, drifts: washDrifts)
                    .environment(artCache)
            }
        }
        .onChange(of: playingRecord, initial: true) { _, _ in
            bleed = player.currentTrackId.map { .track($0) }
        }
        .task(id: tintSource) {
            guard let tintSource, let cover = await art.image(for: tintSource) else {
                recordTint = nil
                return
            }
            recordTint = .dominant(of: cover)
        }
        // Overrides the app-wide tint for everything below, which is every
        // control koan draws itself. What AppKit draws — list selection, focus
        // rings — keeps the declared accent, and that is deliberately a neutral
        // so the two never argue.
        .tint(recordTint ?? .koanAccent)
        .animation(.easeInOut(duration: 2), value: recordTint)
        // The toolbar paints its own ground over whatever is behind it, which
        // put a hard grey strip across the top of a queue washed in the colour
        // of the record. Hidden, the glass controls sit in that colour — which
        // is the whole point of them being glass — and the scroll edge effect
        // keeps rows legible as they pass under.
        .toolbarBackgroundVisibility(.hidden, for: .windowToolbar)
        .onChange(of: search.query) { _, _ in search.schedule() }
        .onSubmit(of: .search) { handleSubmit() }
        // On the window rather than inside the detail column: a
        // `NavigationStack` drops decoration applied around it the moment it
        // pushes, and the transport vanished on every album and artist page.
        // Padded clear of the sidebar instead, because glass floating on glass
        // reads as neither. Each screen makes its own room with
        // `clearsTransport`.
        .overlay(alignment: .bottom) {
            TransportBar()
                .padding(.leading, columns == .detailOnly ? 0 : ui.sidebarWidth)
                .background(
                    GeometryReader { proxy in
                        Color.clear.preference(
                            key: TransportHeightKey.self,
                            value: proxy.size.height
                        )
                    }
                )
        }
        .onPreferenceChange(TransportHeightKey.self) { transportHeight = $0 }
        .onGeometryChange(for: CGSize.self) { $0.size } action: { ui.windowSize = $0 }
        .toolbar {
            // Spans section switches and search jumps, which a NavigationStack's
            // own back button cannot — it only knows about one stack.
            ToolbarItemGroup(placement: .navigation) {
                Button { nav.goBack() } label: {
                    Label("Back", systemImage: "chevron.left")
                }
                .disabled(!nav.canGoBack)
                .help("Back (⌘[)")

                Button { nav.goForward() } label: {
                    Label("Forward", systemImage: "chevron.right")
                }
                .disabled(!nav.canGoForward)
                .help("Forward (⌘])")
            }

            // Separate items with `ToolbarSpacer` between them, not one
            // `ToolbarItemGroup`: a group shares a single pane of glass, which
            // put the filter field and the lyrics toggle in the same capsule.
            ToolbarSpacer(.flexible, placement: .primaryAction)

            // Filtering what is on screen belongs with it, not in the sidebar
            // search, which navigates away instead of narrowing.
            if nav.section == .albums || nav.section == .artists {
                ToolbarItem(placement: .primaryAction) {
                    FilterField(
                        placeholder: nav.section == .albums
                            ? "Filter albums" : "Filter artists",
                        text: $library.filter,
                        focusToken: ui.filterFocusToken
                    )
                    .frame(width: 180)
                }
            }

            // Sort belongs next to what it sorts, so it only appears there.
            if nav.section == .albums {
                // Filtering and sorting are different questions, so they get
                // different panes of glass rather than one joined control.
                ToolbarSpacer(.fixed, placement: .primaryAction)

                // A pull-down with the current choice ticked, the way Finder's
                // arrange control works — rather than a picker forced to a
                // fixed width, which reads as a control that did not fit.
                ToolbarItem(placement: .primaryAction) {
                    Menu {
                        Picker("Sort", selection: Binding(
                            get: { library.albumSort },
                            set: { library.albumSort = $0 }
                        )) {
                            ForEach(AlbumSort.all, id: \.self) { sort in
                                Text(sort.label).tag(sort)
                            }
                        }
                        .pickerStyle(.inline)
                        .labelsHidden()
                    } label: {
                        Label("Sort", systemImage: "arrow.up.arrow.down")
                    }
                    // The accent marks what is playing and what is selected.
                    // A toolbar control that is always there is neither.
                    .tint(.primary)
                    .help("Sort albums — \(library.albumSort.label)")
                }

                // Its own button rather than an item inside the sort menu:
                // reshuffling is something you do repeatedly until you like
                // what you see, and a menu makes that four clicks instead of
                // one.
                if library.albumSort == .random {
                    ToolbarItem(placement: .primaryAction) {
                        Button {
                            library.reshuffleAlbums()
                        } label: {
                            Label("Shuffle", systemImage: "shuffle")
                        }
                        .tint(.primary)
                        .help("Shuffle again")
                    }
                }
            }

        }
        .sheet(isPresented: $ui.showingPicker) {
            PickerSheet(isPresented: $ui.showingPicker)
        }
        // `z`, from wherever you are: the cover in the transport bar opens the
        // same sheet on click, but a keystroke has no cover under the pointer.
        .sheet(isPresented: $ui.showingArtwork) {
            if let trackId = player.currentTrackId {
                ArtworkViewer(
                    source: .track(trackId),
                    title: player.nowPlaying.entry?.title ?? "",
                    subtitle: player.nowPlaying.entry.map { "\($0.artist) — \($0.album)" }
                )
            }
        }
        .sheet(isPresented: $ui.showingShortcuts) {
            ShortcutsSheet(hotkeys: hotkeys.all)
        }
        .overlay(alignment: .bottom) {
            if let error = player.lastError {
                ErrorToast(message: error) { player.lastError = nil }
                    // Above the transport, not behind it.
                    .padding(.bottom, transportHeight + 10)
            }
        }
    }

    /// Return either picks a suggestion — in which case the field holds a token
    /// naming exactly what was chosen — or it means "show me everything".
    private func handleSubmit() {
        guard let selection = SearchModel.Selection(token: search.query) else {
            nav.show(.searchResults)
            return
        }
        switch selection {
        case .track(let id, let albumId):
            // A track lives on its album; that's where you'd play it from.
            if let albumId { nav.jump(to: .album(albumId), highlighting: id) }
        case .album(let id):
            nav.jump(to: .album(id))
        case .artist(let id):
            nav.jump(to: .artist(id))
        }
        search.reset()
    }
}

/// The root of the detail stack.
///
/// A view of its own rather than a `switch` written inline: a `NavigationStack`
/// discards its path when its root changes identity in the same update, and
/// each branch of a `switch` in a `ViewBuilder` is a different view. Keeping
/// the switch one level inside gives the stack a root that never changes, which
/// is what lets the section and the stack move together.
private struct StageView: View {
    @Environment(LibraryModel.self) private var library
    @Environment(Navigator.self) private var nav

    var body: some View {
        switch nav.section {
        case .queue:
            QueueView()
        case .searchResults:
            SearchResultsView()
        case .albums:
            AlbumBrowser()
        case .artists:
            ArtistBrowser()
        case .favourites:
            TrackListView(
                title: "Favourites",
                subtitle: Format.count(Int64(library.favourites.count), "track"),
                tracks: library.favourites
            )
        case .playHistory:
            HistoryView()
        case .snapshots:
            SnapshotsView()
        }
    }
}

/// Engine errors are informational — a device disappearing shouldn't take a
/// modal to dismiss.
private struct ErrorToast: View {
    let message: String
    let dismiss: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: "exclamationmark.circle.fill")
                .foregroundStyle(.orange)
            Text(message)
                .font(.callout)
                .lineLimit(2)
            Button(action: dismiss) {
                Image(systemName: "xmark")
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 11)
        // Tinted glass rather than a material and a border: the tint carries
        // the warning without a second colour, and glass already has an edge.
        .glassEffect(.regular.tint(.orange.opacity(0.22)), in: .capsule)
        .task {
            try? await Task.sleep(for: .seconds(6))
            dismiss()
        }
    }
}


/// The transport bar's rendered height, so the detail column can inset by
/// exactly it rather than by a number someone typed.
private struct TransportHeightKey: PreferenceKey {
    static let defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) {
        value = max(value, nextValue())
    }
}

private extension View {
    /// Room for the transport, which floats over every screen in the stack.
    ///
    /// Measured rather than a constant. The bar's height is a stack of paddings
    /// and a control size, so any number written here would be right until one
    /// of them changed and then be a gap, or a row clipped by a bar with
    /// nothing to say why.
    func clearsTransport(_ height: CGFloat) -> some View {
        // Content passing under the glass is what makes it glass. The soft edge
        // fades a row out as it goes, so one half under the bar reads as behind
        // it rather than cut off.
        safeAreaPadding(.bottom, height)
            .scrollEdgeEffectStyle(.soft, for: .bottom)
    }
}
