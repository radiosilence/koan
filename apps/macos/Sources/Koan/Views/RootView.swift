#if canImport(AppKit)
import AppKit
#endif
import KoanFFI
import SwiftUI

/// Sidebar, stage, optional lyrics inspector, transport pinned to the bottom.
///
/// The stage defaults to the queue rather than the library — koan is a player
/// you build a queue in, and the TUI opens the same way. The library is
/// somewhere you go to feed it.
///
/// The Mac's layout. iOS, iPad included, uses `TabShell`, whose sidebar-adaptable tab
/// bar is the platform's own iPad layout; this one was built for a pointer.
///
/// `NavigationSplitView` is the root and stays the root. Wrapping it in a stack
/// or putting an `HSplitView` in its detail column breaks width propagation:
/// `HSplitView` sizes children to their minimum, so the stage would sit at
/// whatever `minWidth` it declared no matter how large the window got, and an
/// adaptive grid inside it would be stuck at two columns. The lyrics panel is
/// an `inspector` and the transport an overlay on the window for the same
/// reason — both add chrome without taking the detail column's width away.
///
/// The detail column shows one page, chosen by `Navigator`. There is no
/// `NavigationStack`: koan navigates like a browser — any page from any page,
/// with a linear history — and a stack navigates a hierarchy that does not
/// exist here.
#if !os(tvOS)
struct RootView: View {
    /// Single-key shortcuts belong to a machine with a keyboard always attached
    /// — the split view itself does not, which is why this is the only thing in
    /// here the phone cannot have.
    #if os(macOS)
    let hotkeys: Hotkeys
    #endif

    @Environment(UIState.self) private var ui
    @Environment(CoverArtCache.self) private var art
    @Environment(LibraryModel.self) private var library
    @Environment(EngineMirror.self) private var mirror
    @Environment(Navigator.self) private var nav
    @Environment(SearchModel.self) private var search
    /// Held for `reloading` below; nothing on it is read in this body.
    @Environment(PlaylistsModel.self) private var playlists
    /// Held for the closures below — the artwork sheet, the session save.
    /// Nothing on it is read in this body: what is playing is read by the
    /// views that draw it, and by `RecordRoom` for the colour of the window.
    @Environment(PlayerModel.self) private var player

    /// Read for the window's own glass — the toolbar and the transport's soft
    /// edge, which are the platform's rather than koan's.
    @AppStorage("graphics") private var graphics = Graphics.full
    @State private var transportHeight: CGFloat = 0
    /// Watched rather than inferred from the measured width: a collapsed
    /// sidebar still reports its last width, and the transport would keep a
    /// gap where it had been.
    @State private var columns: NavigationSplitViewVisibility = .automatic
    private static var sidebarMin: CGFloat { KoanTheme.metric(215, system: 190) }

    var body: some View {
        @Bindable var ui = ui

        NavigationSplitView(columnVisibility: $columns) {
            SidebarView()
                .modifier(SidebarOverWash())
                .clearsWashedTransport(transportHeight)
                // The column's minimum alone is not held when the window
                // first lays out: the sidebar opened at its content's width
                // and truncated its labels. The content's own minimum is.
                // Wide enough for "recently played" in the theme's face.
                .frame(minWidth: Self.sidebarMin)
                .navigationSplitViewColumnWidth(min: Self.sidebarMin, ideal: Self.sidebarMin + 25, max: 290)
                // The theme draws its own, without the glass; see `PageToolbar`.
                .toolbar(removing: KoanTheme.isOn ? .sidebarToggle : nil)
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
                .clearsWashedTransport(transportHeight)
                .inspectorColumnWidth(min: 260, ideal: 280, max: 460)
                // The toggle belongs to the inspector rather than the window, so
                // it sits at the pane's leading edge and moves with it. In the
                // window's trailing group the pane would open out from
                // underneath it, and it would share a capsule with the filter
                // field.
                .toolbar {
                    ToolbarItem(placement: .primaryAction) {
                        Button {
                            ui.toggleLyrics()
                        } label: {
                            Label("Lyrics", systemImage: Icon.lyrics)
                        }
                        .help("Lyrics panel (⌥⌘L)")
                        .koanButton(.icon)
                    }
                    .sharedBackgroundVisibility(KoanTheme.pane(.automatic))
                }
        }
        // The one place a library change reaches the app's own lists. Every
        // page showing something asked for on demand reloads where it is
        // drawn — see `View.reloading(on:)` — so nothing here decides which
        // model hears what.
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
        // The toolbar paints its own ground over whatever is behind it, a hard
        // grey strip across the top of a queue washed in the colour of the
        // record. Hidden, the glass controls sit in that colour and the scroll
        // edge effect keeps rows legible as they pass under.
        // Restored at `bare`: the ground it paints is opaque, so nothing behind
        // it is sampled and a page switch does not redraw it.
        .koanToolbar(glass: graphics.usesWindowGlass)
        .onSubmit(of: .search) { search.submit() }
        // Backgrounding is the last dependable moment before termination. A
        // notification rather than `scenePhase`: reading that re-runs whatever
        // reads it — here the whole Scene — each time the app loses focus.
        .onReceive(
            NotificationCenter.default.publisher(for: .appResignsActive)
        ) { _ in
            Task { await player.saveOnLeaving() }
        }
        // On the window rather than inside the detail column, padded clear of
        // both columns: glass floating on glass reads as neither, and over the
        // lyrics it hides the last lines of the song. The page makes its own
        // room with `clearsTransport`.
        .overlay(alignment: .bottom) {
            TransportOverlay(columns: columns)
        }
        // The wash and the tint, both the colour of one record. Its own
        // modifier because what it reads moves per track, and a read here
        // re-runs the window — see `RecordRoom`. Outside the transport, which
        // draws in the tint too.
        .modifier(RecordRoom())
        .onPreferenceChange(TransportHeightKey.self) { transportHeight = $0 }
        .onGeometryChange(for: CGSize.self) { $0.size } action: { ui.windowSize = $0 }
        // Its own content rather than built here: it reads the page, and a read in
        // this body would re-run the window on every move.
        .toolbar { PageToolbar() }
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
        #if os(macOS)
        .sheet(isPresented: $ui.showingShortcuts) {
            ShortcutsSheet(hotkeys: hotkeys.all)
        }
        #endif
        .overlay(alignment: .bottom) {
            // Above the transport, not behind it.
            Toasts().padding(.bottom, transportHeight + 10)
        }
    }
}
#endif

/// The filter field, and the only reader of what is typed into it.
///
/// Its own view because the field reads the filter back on every update, and
/// SwiftUI charges that read to whichever body the field sits in. Placed in
/// `RootView` directly, that would be the root: every keystroke would re-run
/// the window and rebuild the toolbar, field and focus with it.
private struct LibraryFilter: View {
    let placeholder: String
    @Environment(LibraryModel.self) private var library
    @Environment(UIState.self) private var ui

    var body: some View {
        @Bindable var library = library
        FilterField(placeholder: placeholder, text: $library.filter, focusToken: ui.filterFocusToken)
    }
}

extension EnvironmentValues {
    /// The colour the room is wearing — what `.tint` was set to, readable.
    ///
    /// SwiftUI offers no way to read a tint back, and an AppKit-backed view
    /// drawing in it has to be handed the colour. Set beside the tint, by the
    /// same modifier, so the two cannot disagree.
    @Entry var roomTint: Color = .koanAccent
    /// Drawn by the evidence renderer, in a window no scene manages.
    @Entry var drawnOffscreen = false
}

/// The room around the page: the wash on the window and the tint on the
/// controls, which are the same answer.
///
/// A modifier rather than lines in `RootView.body`, because what it reads
/// moves per track — the record playing, the page you are on — and a read in
/// the root body is charged to the root, which re-runs the window and rebuilds
/// the toolbar with it, throwing away the filter field and the focus in it. A
/// modifier's body is its own. `content` is the window already built, and the
/// tint reaches it as an environment change that only what draws in it sees.
///
/// The record the room takes its colour from: a page about one record answers
/// with it — an album with its own sleeve, a playlist with the first of its
/// records, the same one that leads its mosaic. Every other page — a grid, a
/// list of artists, favourites, history — is not about any record in
/// particular, so it answers with the one playing. The room is coloured by the
/// music wherever you have wandered off to, and only a page that disagrees
/// says otherwise.
struct RecordRoom: ViewModifier {
    @Environment(Navigator.self) private var nav
    @Environment(PlayerModel.self) private var player
    @Environment(CoverArtCache.self) private var art
    /// Read for the wash a playlist page sits in: its colour is the first
    /// record in it, since a playlist has no cover of its own.
    @Environment(PlaylistsModel.self) private var playlists
    @Environment(\.drawnOffscreen) private var offscreen
    @Environment(AppearanceModel.self) private var appearance
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    /// The colour of a record the cache could not already answer for, and which
    /// record it was worked out for. Only consulted when the cache cannot.
    @State private var fetchedTint: (source: AlbumArtwork.Source, colour: Color?)?
    /// The colour the room is wearing, kept on while the next one is worked
    /// out. Falling back to the accent instead flashes every tinted control to
    /// it and back again on the way to a record whose colour is not in yet.
    @State private var worn: Color?

    /// Only for a colour that had to be worked out, which arrives after the page
    /// and would otherwise cut. A colour already in hand needs no ease: it lands
    /// in the same frame as the record it belongs to, which is what an ease was
    /// standing in for.
    ///
    /// It is deliberately not on the common path. A tint is a value every
    /// control reads rather than a property of a layer, so the compositor
    /// cannot take this one — easing it over two seconds is a hundred and
    /// twenty renders of the whole window, each one a commit, and each commit a
    /// synchronous round trip to the render server.
    ///
    /// The kōan theme eases for a quarter of a second (`Motion.settle`): its
    /// accent is the colour of every selection and indicator, and a two-second
    /// drift there reads as something wrong rather than a room changing.
    @MainActor private static var tintEase: Animation {
        KoanTheme.isOn ? KoanTheme.Motion.settle : .easeInOut(duration: 2)
    }

    /// Read straight through the cache on every pass, the way `AlbumArtwork`
    /// reads its bitmap: a colour the app already holds lands in the same commit
    /// as the page that wanted it. Held in `@State` and written by a task, it
    /// would be a second commit every time — the page, and then the room around it.
    ///
    /// Doubly optional, as `ArtworkBleed.answered` is: the outer `nil` is a
    /// colour not worked out yet, the inner one a record with none.
    private var recordTint: Color?? {
        guard let colourSource else { return .some(nil) }
        if let held = art.cachedColour(for: colourSource) { return held }
        guard let fetchedTint, fetchedTint.source == colourSource else { return nil }
        return .some(fetchedTint.colour)
    }

    /// The record's colour once it is known, and until then the one already
    /// on; `nil` is a record with none, nothing playing, or colours from the
    /// record turned off.
    private var record: Color? {
        guard appearance.recordColours else { return nil }
        return switch recordTint {
        case .some(let colour): colour
        case .none: worn
        }
    }

    /// The accent for that record, tone-mapped to its bands — in either look.
    private var accent: KoanAccent { KoanAccent.of(record) }

    /// The colour to put on.
    private var tint: Color { accent.color }

    private var colourSource: AlbumArtwork.Source? {
        switch nav.current {
        case .album(let id): .album(id)
        case .section(.playlist(let id)): playlists.covers[id]?.first ?? player.currentArtwork
        default: player.currentArtwork
        }
    }

    func body(content: Content) -> some View {
        // The window background is evaluated by the *scene*, outside every
        // environment this was handed, so anything it needs is captured here.
        // Reading an `@Environment` inside that closure — including to put one
        // back — traps, and the app dies on launch.
        let wash = colourSource
        // Whether this pass had to guess. The cache is not observed, so a
        // colour landing in it later — worked out by `LibraryModel.warm`
        // alongside the rows — re-renders nothing; the task below hands it
        // over instead.
        let guessed = recordTint == nil
        let player = player
        let artCache = art
        let appearanceModel = appearance
        // Over an opaque ground, because this *replaces* the window's own
        // background rather than sitting on it — a half-transparent wash on its
        // own leaves you looking through the app at the desktop.
        let washLayer = ZStack {
            Rectangle().fill(KoanTheme.ground(.background))
            WindowWash(source: wash, player: player)
                .environment(artCache)
                .environment(appearanceModel)
        }

        content
            // The queue is a list of names, and the record playing is the only
            // thing in it with a colour. On the *window* rather than behind
            // the queue: nothing inside a split view column reaches past the
            // toolbar's inset, and a wash that stops in a line under the
            // toolbar is worse than none. An album page washes its own header,
            // so the window stays out of its way.
            // A window on the Mac, the navigation container on iOS: the same
            // intent, and neither platform has the other's container.
            #if os(macOS)
            .containerBackground(for: .window) { washLayer }
            // A window the renderer draws has no scene to hand that to.
            .background { if offscreen { washLayer.ignoresSafeArea() } }
            #elseif os(tvOS)
            .background { washLayer.ignoresSafeArea() }
            #else
            .containerBackground(for: .navigation) { washLayer }
            #endif
            // Only for a record whose colour is not already known. The usual
            // path is answered above, in the same pass as the page —
            // navigating warms this alongside the rows, see
            // `LibraryModel.prepare(album:)`.
            .task(id: colourSource) {
                guard let colourSource else { return }
                if let held = art.cachedColour(for: colourSource) {
                    // Worked out between this pass reading the cache and the
                    // task starting. Without this the room keeps the colour it
                    // was wearing — the accent, on a first visit — for as long
                    // as nothing else happens to redraw it.
                    if guessed { fetchedTint = (colourSource, held) }
                    return
                }
                // Nobody is waiting on a slow ease into the background, so it
                // stands aside until the page in front of it has drawn rather
                // than racing it for artwork, threads and a slot on the main
                // actor.
                try? await Task.sleep(for: .milliseconds(150))
                let colour = await art.dominantColour(for: colourSource)
                guard !Task.isCancelled else { return }
                withAnimation(reduceMotion ? nil : Self.tintEase) { fetchedTint = (colourSource, colour) }
            }
            // Overrides the app-wide tint for everything below, which is every
            // control koan draws itself. What AppKit draws — list selection,
            // focus rings — keeps the declared accent, and that is deliberately
            // a neutral so the two never argue. A television's alerts take the
            // tint too, as text on their white focused button, so there the
            // colour goes to the room alone.
            #if !os(tvOS)
            .tint(tint)
            #endif
            .environment(\.roomTint, tint)
            .environment(\.koanAccent, accent)
            // The theme's text button for every button that names no style.
            // Not on a television, whose shell gives them `TelevisionButton`:
            // a bare text button there shows no focus.
            #if !os(tvOS)
            .koanButtons(.text)
            #endif
            .onChange(of: record, initial: true) { _, now in worn = now }
    }
}

/// The transport, padded clear of the columns.
///
/// Its own view because the widths it reads move on every frame the sidebar
/// is being dragged — read in the root, each of those frames would re-run
/// the window.
private struct TransportOverlay: View {
    let columns: NavigationSplitViewVisibility

    @Environment(UIState.self) private var ui
    @Environment(AppearanceModel.self) private var appearance: AppearanceModel?

    var body: some View {
        // Over the wash it is the window's foot, under every column, as the
        // phone's mini player is.
        let washed = KoanTheme.washesWindow(appearance)
        TransportBar()
            .padding(.leading, washed || columns == .detailOnly ? 0 : ui.sidebarWidth)
            .padding(.trailing, !washed && ui.showLyrics ? ui.lyricsWidth : 0)
            .background(
                GeometryReader { proxy in
                    Color.clear.preference(
                        key: TransportHeightKey.self,
                        value: proxy.size.height
                    )
                }
            )
    }
}

/// Select, or what to do with what has been selected. The only reader of the
/// selection outside the tiles, so a tick re-runs this and not the root.
private struct SelectionControls: View {
    let selection: PlayableSelection

    @Environment(LibraryModel.self) private var library
    @Environment(PlayerModel.self) private var player

    var body: some View {
        if selection.isActive {
            let count = selection.picked.count
            HStack(spacing: 2) {
                Button {
                    selection.commit(engine: library.engine, player: player, play: true)
                } label: {
                    Label(count > 0 ? "Play \(count)" : "Play", systemImage: Icon.play)
                        .labelStyle(.titleAndIcon)
                }
                .disabled(count == 0)
                .help("Play the selection, replacing the queue")
                Button {
                    selection.commit(engine: library.engine, player: player, play: false)
                } label: {
                    Label(count > 0 ? "Add \(count) to Queue" : "Add to Queue", systemImage: Icon.queue)
                        .labelStyle(.titleAndIcon)
                }
                .disabled(count == 0)
                .help("Add the selection to the end of the queue")
                Button("Done") { selection.end() }
                    .help("Stop selecting (Esc)")
            }
        } else {
            Button {
                selection.begin()
            } label: {
                Label("Select", systemImage: Icon.selectAll)
            }
            .help("Pick several to play or queue (⌘-click one, or ⌘A)")
        }
    }
}

/// The wash behind the window, reading whether anything is playing itself —
/// play and pause change how it breathes and nothing else about the window.
///
/// Handed the model rather than a value taken from it: taken in `RootView`, the
/// read would be the root's, and every pause would re-run the whole window.
private struct WindowWash: View {
    let source: AlbumArtwork.Source?
    let player: PlayerModel

    var body: some View {
        ArtworkBleed(source: source, drifts: player.isPlaying)
    }
}

/// Its own view so that a toast coming and going is read here, not by the root.
private struct Toasts: View {
    @Environment(PlayerModel.self) private var player

    var body: some View {
        // One slot, and a failure outranks a remark about something that has
        // not finished yet.
        if let error = player.lastError {
            ErrorToast(message: error) { player.lastError = nil }
        } else if let notice = player.lastNotice {
            ErrorToast(message: notice, kind: .notice) { player.lastNotice = nil }
        }
    }
}

/// The page. One `switch`, no stack.
private struct StageView: View {
    @Environment(Navigator.self) private var nav

    /// The queue is never torn down; every other page is built when you arrive
    /// and thrown away when you leave.
    ///
    /// The queue keeps its place that way. The album and artist browsers are
    /// rebuilt and put back where they were instead (see `AlbumBrowser` and
    /// `ArtistBrowser`): a page kept mounted is still laid out with the window,
    /// and two browsers of thousands of rows kept behind the page on screen
    /// made every page switch pay to lay them out again.
    ///
    /// Off stage the queue is invisible, untouchable, unfocusable and told so,
    /// which is what stops the row that is playing animating behind a page you
    /// are looking at.
    var body: some View {
        ZStack {
            QueueView()
                .staged(nav.current == .section(.queue))

            if let page = pageOnStage {
                page
            }
        }
    }

    /// The page on screen when it is not the queue.
    private var pageOnStage: AnyView? {
        switch nav.current {
        case .section(.queue):
            return nil
        // A browser goes back to its top by scrolling, not by being rebuilt,
        // since rebuilt it would put itself back where it was.
        case .section(let section) where section == .albums || section == .artists:
            return AnyView(page(section))
        case .section(let section):
            return AnyView(page(section).id(nav.rewinds[section, default: 0]))
        case .album(let id):
            return AnyView(AlbumDetailView(albumId: id))
        case .artist(let id):
            return AnyView(ArtistDetailView(artistId: id))
        }
    }

    @ViewBuilder private func page(_ section: Navigator.Section) -> some View {
        switch section {
        case .searchResults: SearchResultsView()
        case .favourites: FavouritesView()
        case .recentlyPlayed: RecentlyPlayedView()
        case .onDevice: OnDeviceView()
        case .playHistory: HistoryView()
        case .downloads: DownloadsView()
        case .playlist(let id): PlaylistView(playlistId: id)
        case .albums: AlbumBrowser()
        case .artists: ArtistBrowser()
        case .tracks: TrackBrowser()
        case .queue: EmptyView()
        }
    }
}

private extension View {
    /// On stage, or kept mounted behind whatever is.
    func staged(_ onStage: Bool) -> some View {
        opacity(onStage ? 1 : 0)
            .allowsHitTesting(onStage)
            .disabled(!onStage)
            .accessibilityHidden(!onStage)
            .environment(\.onStage, onStage)
    }
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
                .font(.role(.control, system: .callout))
                .lineLimit(2)
            Button(action: dismiss) {
                Image(systemName: "xmark")
            }
            .buttonStyle(.plain)
            .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
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
        // Restarted by a new message; a cancelled sleep is not a timeout.
        .task(id: message) {
            guard (try? await Task.sleep(for: .seconds(6))) != nil else { return }
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

/// Room for the transport, which floats over every screen in the stack.
///
/// Measured rather than a constant. The bar's height is a stack of paddings
/// and a control size, so any number written here would be right until one
/// of them changed and then be a gap, or a row clipped by a bar with
/// nothing to say why.
private struct ClearsTransport: ViewModifier {
    let height: CGFloat
    let glass: Bool
    @Environment(AppearanceModel.self) private var appearance: AppearanceModel?

    func body(content: Content) -> some View {
        if KoanTheme.washesWindow(appearance) {
            content.modifier(StopsAtBars(bottom: height))
        } else {
            // Content passing under the glass is what makes it glass. The soft
            // edge fades a row out as it goes, so one half under the bar reads
            // as behind it rather than cut off — and it is a live blur of a
            // window-wide strip, which is why `bare` does without it and takes
            // the hard edge instead.
            content
                .safeAreaPadding(.bottom, height)
                .scrollEdgeEffectStyle(glass ? .soft : .hard, for: .bottom)
        }
    }
}

/// In the washed theme the toolbar sits on the wash with nothing under it and
/// a hairline at the edge it shares with the page, and the transport on its
/// own ground below a rule, so a page stops at those edges: a row passing
/// beneath the toolbar would need a scrim or a fade to be told from its text. The toolbar's safe area
/// becomes real space, so the AppKit lists that scroll into a safe area find
/// none at the top, and everything past the edges is clipped.
private struct StopsAtBars: ViewModifier {
    let bottom: CGFloat
    @State private var top: CGFloat = 0

    func body(content: Content) -> some View {
        content
            .padding(.top, top)
            .padding(.bottom, bottom)
            .clipped()
            // The toolbar's lower edge, as the transport's upper one is drawn.
            .overlay(alignment: .top) {
                Rectangle().fill(Color.koanRowRule).frame(height: KoanTheme.hairline).padding(.top, top)
            }
            .ignoresSafeArea(.container, edges: .top)
            .background {
                Color.clear.onGeometryChange(for: CGFloat.self) { $0.safeAreaInsets.top } action: { top = $0 }
            }
    }
}

/// The sidebar column on the wash, in the theme with the wash under the whole
/// window: no glass of the platform's, and a hairline where it meets the page.
private struct SidebarOverWash: ViewModifier {
    @Environment(AppearanceModel.self) private var appearance: AppearanceModel?

    func body(content: Content) -> some View {
        let washed = KoanTheme.washesWindow(appearance)
        content
            #if os(macOS)
            .background(SidebarGround(themed: KoanTheme.isOn))
            #endif
            .overlay(alignment: .trailing) {
                if washed {
                    Rectangle().fill(Color.koanRowRule).frame(width: KoanTheme.hairline)
                        .ignoresSafeArea()
                }
            }
    }
}

/// The sidebar and the lyrics in the washed theme, where the transport runs
/// under them too: they stop at its edge as the page does.
private struct ClearsWashedTransport: ViewModifier {
    let height: CGFloat
    @Environment(AppearanceModel.self) private var appearance: AppearanceModel?

    func body(content: Content) -> some View {
        content.padding(.bottom, KoanTheme.washesWindow(appearance) ? height : 0)
    }
}

private extension View {
    func clearsTransport(_ height: CGFloat, glass: Bool) -> some View {
        modifier(ClearsTransport(height: height, glass: glass))
    }

    func clearsWashedTransport(_ height: CGFloat) -> some View {
        modifier(ClearsWashedTransport(height: height))
    }
}


/// The window's toolbar: back and forward, then the controls for the page.
///
/// The same items on every page. A control that does not apply to a page is
/// not drawn there, but its item stays: adding or removing an item makes AppKit
/// re-tile the toolbar, and a re-tile lays out the whole window — every page
/// kept mounted behind the one on screen included, which is most of what a
/// page switch cost.
#if !os(tvOS)
private struct PageToolbar: ToolbarContent {
    @Environment(Navigator.self) private var nav
    @Environment(LibraryModel.self) private var library
    @Environment(SearchModel.self) private var search

    var body: some ToolbarContent {
        // Back and forward walk the pages you visited, in order, wherever they
        // were.
        ToolbarItemGroup(placement: .navigation) {
            // The theme's own sidebar toggle, the system's being a capsule of
            // glass; see `RootView`, which takes the system's away.
            #if os(macOS)
            if KoanTheme.isOn {
                Button {
                    NSApp.sendAction(#selector(NSSplitViewController.toggleSidebar(_:)), to: nil, from: nil)
                } label: {
                    Label("Sidebar", systemImage: "sidebar.left")
                }
                .help("Show or hide the sidebar (⌃⌘S)")
                .koanButton(.icon)
            }
            #endif
            Button { nav.goBack() } label: {
                Label("Back", systemImage: Icon.back)
            }
            .disabled(!nav.canGoBack)
            .help("Back (⌘[)")
            .koanButton(.icon)

            Button { nav.goForward() } label: {
                Label("Forward", systemImage: Icon.forward)
            }
            .disabled(!nav.canGoForward)
            .help("Forward (⌘])")
            .koanButton(.icon)
        }
        .sharedBackgroundVisibility(KoanTheme.pane(.automatic))

        // Separate items with `ToolbarSpacer` between them, not one
        // `ToolbarItemGroup`: a group shares a single pane of glass, which
        // would put the filter field and the lyrics toggle in the same capsule.
        ToolbarSpacer(.flexible, placement: .primaryAction)

        // Filtering what is on screen belongs with it, not in the sidebar
        // search, which navigates away instead of narrowing.
        ToolbarItem(placement: .primaryAction) {
            if let placeholder = nav.section?.filterPlaceholder {
                LibraryFilter(placeholder: placeholder)
                    .frame(width: 180)
            }
        }
        .sharedBackgroundVisibility(KoanTheme.pane(nav.section?.filterPlaceholder == nil ? .hidden : .automatic))

        // Sort and filters belong next to what they narrow, so they only
        // appear there. Typing a name and picking from a menu are different
        // gestures, so the field gets its own pane of glass and the two
        // buttons share another.
        ToolbarSpacer(.fixed, placement: .primaryAction)

        ToolbarItem(placement: .primaryAction) {
            if nav.section?.isBrowser == true {
                HStack(spacing: 2) {
                    BrowseFilterButton()
                    if nav.section == .albums {
                        AlbumSortControls()
                    }
                    if nav.section == .tracks {
                        TrackSortControls()
                    }
                }
                .koanButtons(.icon)
            }
        }
        .sharedBackgroundVisibility(KoanTheme.pane(nav.section?.isBrowser == true ? .automatic : .hidden))

        // Last, and apart from the filter: what you do with a pick is not part
        // of narrowing the grid, and next to the field the two read as one
        // control.
        ToolbarSpacer(.fixed, placement: .primaryAction)

        ToolbarItem(placement: .primaryAction) {
            if let selection {
                SelectionControls(selection: selection)
            }
        }
        .sharedBackgroundVisibility(KoanTheme.pane(selection == nil ? .hidden : .automatic))
    }

    /// The pick the page on screen makes, if it makes one: the album grid, an
    /// artist's records and search results are picked the same way.
    private var selection: PlayableSelection? {
        if nav.section == .albums { return library.selection }
        if case .artist = nav.current { return library.artistSelection }
        if nav.section == .searchResults { return search.selection }
        return nil
    }
}
#endif

/// The album grid's sort, and reshuffling when the sort is random.
private struct AlbumSortControls: View {
    @Environment(LibraryModel.self) private var library

    var body: some View {
        HStack(spacing: 2) {
            // A pull-down with the current choice ticked, the way Finder's
            // arrange control works — rather than a picker forced to a fixed
            // width, which reads as a control that did not fit.
            Menu {
                Picker("Sort", selection: Binding(
                    get: { library.albumSort },
                    set: { library.albumSort = $0 }
                )) {
                    ForEach(AlbumSort.offered(
                    recent: library.browseFilter.recent, downloaded: library.browseFilter.downloaded,
                    searching: !library.filter.isEmpty
                ), id: \.self) { sort in
                        Text(sort.label).tag(sort)
                    }
                }
                .pickerStyle(.inline)
                .labelsHidden()
            } label: {
                Label("Sort", systemImage: "arrow.up.arrow.down")
            }
            // The accent marks what is playing and what is selected. A toolbar
            // control that is always there is neither.
            .tint(.primary)
            .help("Sort albums — \(library.albumSort.label)")

            // Its own button rather than an item inside the sort menu:
            // reshuffling is something you do repeatedly until you like what
            // you see, and a menu makes that four clicks instead of one.
            if library.albumSort == .random {
                Button {
                    library.reshuffleAlbums()
                } label: {
                    Label("Shuffle", systemImage: Icon.reshuffle)
                }
                .tint(.primary)
                .help("Shuffle again")
            }
        }
    }
}

/// The track browser's sort.
private struct TrackSortControls: View {
    @Environment(LibraryModel.self) private var library

    var body: some View {
        Menu {
            Picker("Sort", selection: Binding(
                get: { library.trackSort },
                set: { library.trackSort = $0 }
            )) {
                ForEach(TrackBrowseSort.offered(recent: library.browseFilter.recent, searching: !library.filter.isEmpty), id: \.self) { sort in
                    Text(sort.label).tag(sort)
                }
            }
            .pickerStyle(.inline)
            .labelsHidden()
        } label: {
            Label("Sort", systemImage: "arrow.up.arrow.down")
        }
        .tint(.primary)
        .help("Sort tracks — \(library.trackSort.label)")
    }
}

private extension View {
    /// A toolbar control that stays laid out on pages it does not apply to,
    /// unseen and untouchable there. The toolbar keeps the same geometry on
    /// every page, so moving between pages never makes AppKit re-tile it.
    func slot(applies: Bool) -> some View {
        opacity(applies ? 1 : 0)
            .allowsHitTesting(applies)
            .disabled(!applies)
            .accessibilityHidden(!applies)
    }
}
