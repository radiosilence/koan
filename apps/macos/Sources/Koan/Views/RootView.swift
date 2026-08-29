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
/// The detail column shows one page, chosen by `Navigator`. There is no
/// `NavigationStack`: koan navigates like a browser — any page from any page,
/// with a linear history — and a stack navigates a hierarchy that does not
/// exist here.
struct RootView: View {
    let hotkeys: Hotkeys

    @Environment(UIState.self) private var ui
    @Environment(LibraryModel.self) private var library
    @Environment(Navigator.self) private var nav
    @Environment(SearchModel.self) private var search
    @Environment(PlayerModel.self) private var player
    /// Read here only to hand back to the window background — see below.
    @Environment(CoverArtCache.self) private var art
    /// Read for the wash a playlist page sits in: its colour is the first
    /// record in it, since a playlist has no cover of its own.
    @Environment(PlaylistsModel.self) private var playlists

    /// Read for the window's own glass — the toolbar and the transport's soft
    /// edge, which are the platform's rather than koan's and which no step of
    /// this setting used to reach.
    @AppStorage("graphics") private var graphics = Graphics.full
    @State private var transportHeight: CGFloat = 0
    /// The colour of a record the cache could not already answer for, and which
    /// record it was worked out for. Only consulted when the cache cannot.
    @State private var fetchedTint: (source: AlbumArtwork.Source, colour: Color?)?

    /// Only for a colour that had to be worked out, which arrives after the page
    /// and would otherwise cut. A colour already in hand needs no ease: it lands
    /// in the same frame as the record it belongs to, which is what an ease was
    /// standing in for.
    ///
    /// It is deliberately not on the common path. A tint is a value every
    /// control reads rather than a property of a layer, so the compositor
    /// cannot take this one — easing it over two seconds is a hundred and
    /// twenty renders of the whole window, each one a commit, and each commit a
    /// synchronous round trip to the render server. That was half of what
    /// opening a record cost.
    private static let tintEase = Animation.easeInOut(duration: 2)
    /// Watched rather than inferred from the measured width: a collapsed
    /// sidebar still reports its last width, so the transport kept a gap where
    /// it used to be.
    @State private var columns: NavigationSplitViewVisibility = .automatic

    /// The record the room takes its colour from — both the wash on the window
    /// and the tint on the controls, which are the same answer and were once
    /// two.
    ///
    /// A page about one record answers with it: an album with its own sleeve, a
    /// playlist with the first of its records, the same one that leads its
    /// mosaic. Every other page — a grid, a list of artists, favourites,
    /// history — is not about any record in particular, so it answers with the
    /// one playing. The room is coloured by the music wherever you have
    /// wandered off to, and only a page that disagrees says otherwise.
    /// Read straight through the cache on every pass, the way `AlbumArtwork`
    /// reads its bitmap: a colour the app already holds lands in the same commit
    /// as the page that wanted it. Held in `@State` and written by a task, it
    /// was a second commit every time — the page, and then the room around it.
    private var recordTint: Color? {
        guard let colourSource else { return nil }
        if let held = art.cachedColour(for: colourSource) { return held }
        guard let fetchedTint, fetchedTint.source == colourSource else { return nil }
        return fetchedTint.colour
    }

    private var colourSource: AlbumArtwork.Source? {
        switch nav.current {
        case .album(let id): .album(id)
        case .section(.playlist(let id)): playlists.covers[id]?.first ?? player.currentArtwork
        default: player.currentArtwork
        }
    }

    var body: some View {
        @Bindable var library = library
        @Bindable var search = search
        @Bindable var ui = ui

        // The window background is evaluated by the *scene*, outside every
        // environment `RootView` was handed, so anything it needs is captured
        // here. Reading an `@Environment` inside that closure — including to
        // put one back — traps, and the app dies on launch.
        let wash = colourSource
        let washDrifts = player.isPlaying
        let artCache = art

        NavigationSplitView(columnVisibility: $columns) {
            SidebarView()
                .navigationSplitViewColumnWidth(min: 190, ideal: 215, max: 290)
        } detail: {
            StageView()
                .clearsTransport(transportHeight, glass: graphics.usesWindowGlass)
                // A page fills the column whether or not it has anything in it
                // to fill it with. Results while the query is still running
                // measure nothing, and an unfilled page leaves the transport
                // and the scroll edges sized to it.
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .inspector(isPresented: $ui.showLyrics) {
            LyricsPanel()
                .inspectorColumnWidth(min: 260, ideal: 320, max: 460)
                // The column animates on its own; its contents do not come
                // with it. Without this the stage slides over and the pane
                // then appears whole in one frame, a fifth of a second later.
                .transition(.move(edge: .trailing))
                // The toggle belongs to the inspector rather than the window, so
                // it sits at the pane's leading edge and moves with it. In the
                // window's trailing group the pane opened out from underneath
                // it, and it shared a capsule with the filter field.
                .toolbar {
                    ToolbarItem(placement: .primaryAction) {
                        Button {
                            ui.toggleLyrics()
                        } label: {
                            Label("Lyrics", systemImage: Icon.lyrics)
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
        // The one place a library change reaches the app's own lists. Every
        // page showing something asked for on demand reloads where it is
        // drawn — see `View.reloading(on:)` — so nothing here decides which
        // model hears what.
        .reloading(on: 0) {
            library.libraryChanged()
            playlists.load()
        }
        // Only for a record whose colour is not already known. The usual path
        // is answered above, in the same pass as the page — navigating warms
        // this alongside the rows, see `LibraryModel.prepare(album:)`.
        .task(id: colourSource) {
            guard let colourSource, art.cachedColour(for: colourSource) == nil else { return }
            // Nobody is waiting on a slow ease into the background, so it stands
            // aside until the page in front of it has drawn rather than racing
            // it for artwork, threads and a slot on the main actor.
            try? await Task.sleep(for: .milliseconds(150))
            let colour = await art.dominantColour(for: colourSource)
            guard !Task.isCancelled else { return }
            withAnimation(Self.tintEase) { fetchedTint = (colourSource, colour) }
        }
        // Overrides the app-wide tint for everything below, which is every
        // control koan draws itself. What AppKit draws — list selection, focus
        // rings — keeps the declared accent, and that is deliberately a neutral
        // so the two never argue.
        .tint(recordTint ?? .koanAccent)
        // The toolbar paints its own ground over whatever is behind it, which
        // put a hard grey strip across the top of a queue washed in the colour
        // of the record. Hidden, the glass controls sit in that colour — which
        // is the whole point of them being glass — and the scroll edge effect
        // keeps rows legible as they pass under.
        // Restored at `bare`: the ground it paints is opaque, so nothing behind
        // it is sampled and a page switch does not redraw it.
        .toolbarBackgroundVisibility(
            graphics.usesWindowGlass ? .hidden : .automatic, for: .windowToolbar
        )
        .onChange(of: search.query) { _, _ in search.schedule() }
        .onSubmit(of: .search) { handleSubmit() }
        // On the window rather than inside the detail column, padded clear of
        // both columns: glass floating on glass reads as neither, and over the
        // lyrics it hides the last lines of the song. The page makes its own
        // room with `clearsTransport`.
        .overlay(alignment: .bottom) {
            TransportBar()
                .padding(.leading, columns == .detailOnly ? 0 : ui.sidebarWidth)
                .padding(.trailing, ui.showLyrics ? ui.lyricsWidth : 0)
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
            // Back and forward walk the pages you actually visited, in order,
            // wherever they were.
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
            if let placeholder = nav.section?.filterPlaceholder {
                ToolbarItem(placement: .primaryAction) {
                    FilterField(
                        placeholder: placeholder,
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
        // Named here, not where it was asked for: most of the things that ask
        // are context menus, and a menu takes its own alerts down with it.
        .newPlaylistAlert()
        .sheet(isPresented: $ui.showingPicker) {
            PickerSheet(isPresented: $ui.showingPicker)
        }
        // `z`, from wherever you are: the cover in the transport bar opens the
        // same sheet on click, but a keystroke has no cover under the pointer.
        .sheet(isPresented: $ui.showingArtwork) {
            if let sleeve = player.currentArtwork {
                ArtworkViewer(
                    source: sleeve,
                    title: player.currentEntry?.title ?? "",
                    subtitle: player.currentEntry.map { "\($0.artist) — \($0.album)" }
                )
            }
        }
        .sheet(isPresented: $ui.showingShortcuts) {
            ShortcutsSheet(hotkeys: hotkeys.all)
        }
        .overlay(alignment: .bottom) {
            // One slot, and a failure outranks a remark about something that
            // has not finished yet.
            if let error = player.lastError {
                ErrorToast(message: error) { player.lastError = nil }
                    // Above the transport, not behind it.
                    .padding(.bottom, transportHeight + 10)
            } else if let notice = player.lastNotice {
                ErrorToast(message: notice, kind: .notice) { player.lastNotice = nil }
                    .padding(.bottom, transportHeight + 10)
            }
        }
    }

    /// Return either picks a suggestion — in which case the field holds a token
    /// naming exactly what was chosen — or it means "show me everything".
    private func handleSubmit() {
        // Emptying the field submits it again. Acting on that sent you to the
        // results page for a search you had not asked for — and since clearing
        // the query then forgets that page, you landed on whatever list was
        // behind it, one keystroke after picking an album.
        let query = search.query.trimmingCharacters(in: .whitespaces)
        guard !query.isEmpty else { return }

        guard let selection = SearchModel.Selection(token: query) else {
            nav.show(.searchResults)
            return
        }
        switch selection {
        case .track(let id, let albumId):
            // A track lives on its album; that's where you'd play it from.
            if let albumId { nav.open(album: albumId, highlighting: id) }
        case .album(let id):
            nav.open(album: id)
        case .artist(let id):
            nav.open(artist: id)
        }
        search.reset()
    }
}

/// The page. One `switch`, no stack.
///
private struct StageView: View {
    @Environment(LibraryModel.self) private var library
    @Environment(Navigator.self) private var nav

    private var onQueue: Bool { nav.current == .section(.queue) }

    /// The queue is never torn down; every other page is built when you arrive
    /// and thrown away when you leave.
    ///
    /// That asymmetry buys the one thing a `List` cannot be given back: where it
    /// was scrolled to. On macOS a `List` is AppKit's table, and every SwiftUI
    /// way of asking one to go to an offset — `scrollPosition`, `scrollTo(y:)`,
    /// `scrollPosition(id:)` — is inert on it, so a queue rebuilt on the way
    /// back always starts at the top. Keeping it mounted means it never left.
    ///
    /// Off stage it is invisible, untouchable, unfocusable and told so, which
    /// is what stops the row that is playing animating behind a page you are
    /// actually looking at.
    var body: some View {
        ZStack {
            QueueView()
                .opacity(onQueue ? 1 : 0)
                .allowsHitTesting(onQueue)
                .disabled(!onQueue)
                .accessibilityHidden(!onQueue)
                .environment(\.onStage, onQueue)

            if !onQueue { page }
        }
    }

    @ViewBuilder private var page: some View {
        switch nav.current {
        case .section(.queue):
            // Kept alive above, and this is only reached when it is not showing.
            EmptyView()
        case .section(.searchResults):
            SearchResultsView()
        case .section(.albums):
            AlbumBrowser()
        case .section(.artists):
            ArtistBrowser()
        case .section(.favourites):
            FavouritesView()
        case .section(.playHistory):
            HistoryView()
        case .section(.downloads):
            DownloadsView()
        case .section(.playlist(let id)):
            PlaylistView(playlistId: id)
        case .album(let id):
            AlbumDetailView(albumId: id)
        case .artist(let id):
            ArtistDetailView(artistId: id)
        }
    }
}

extension EnvironmentValues {
    /// Whether the page this view belongs to is the one on screen. False only
    /// for the queue while you are somewhere else — see `StageView`. Anything
    /// that animates or subscribes to keep itself current reads it.
    @Entry var onStage = true
}

/// Engine errors are informational — a device disappearing shouldn't take a
/// modal to dismiss.
private struct ErrorToast: View {
    /// Whether something went wrong, or something is merely not available yet.
    /// The second is not a warning and does not get the colour of one — a
    /// track that is still downloading is working exactly as intended.
    enum Kind {
        case warning
        case notice

        var symbol: String {
            switch self {
            case .warning: "exclamationmark.circle.fill"
            case .notice: "arrow.down.circle.fill"
            }
        }

        var tint: Color {
            switch self {
            case .warning: .orange
            case .notice: .secondary
            }
        }
    }

    let message: String
    var kind: Kind = .warning
    let dismiss: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: kind.symbol)
                .foregroundStyle(kind.tint)
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
        .glass(
            .regular.tint(kind.tint.opacity(0.22)),
            fallback: kind.tint.opacity(0.22),
            in: .capsule
        )
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
    func clearsTransport(_ height: CGFloat, glass: Bool) -> some View {
        // Content passing under the glass is what makes it glass. The soft edge
        // fades a row out as it goes, so one half under the bar reads as behind
        // it rather than cut off — and it is a live blur of a window-wide strip,
        // which is why `bare` does without it and takes the hard edge instead.
        safeAreaPadding(.bottom, height)
            .scrollEdgeEffectStyle(glass ? .soft : .hard, for: .bottom)
    }
}
