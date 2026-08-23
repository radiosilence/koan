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
/// There is one `NavigationStack` for the whole detail column, and its path
/// lives on `LibraryModel`. Per-browser stacks can't be driven from outside,
/// and search needs to push you into one.
struct RootView: View {
    @Binding var showingPicker: Bool

    @Environment(LibraryModel.self) private var library
    @Environment(SearchModel.self) private var search
    @Environment(PlayerModel.self) private var player

    @AppStorage("showLyrics") private var showLyrics = false
    @State private var transportHeight: CGFloat = 0

    var body: some View {
        @Bindable var library = library
        @Bindable var search = search

        NavigationSplitView {
            SidebarView()
                .navigationSplitViewColumnWidth(min: 190, ideal: 215, max: 290)
        } detail: {
            NavigationStack(path: $library.path) {
                stage
                    // The transport is a safeAreaInset on the split view, which
                    // reserves space in the *window* but not inside the detail
                    // column's own scroll views — a List draws its last rows
                    // under the bar, where they cannot be read or clicked.
                    //
                    // Measured rather than a constant. The bar's height is a
                    // stack of paddings and a control size, so any number
                    // written here would be right until one of them changed and
                    // then be a gap or a clipped row with nothing to say why.
                    .safeAreaPadding(.bottom, transportHeight)
                    // The stack draws its own back button for pushed
                    // destinations, next to the pair we already have — three
                    // chevrons in a row. Ours can cross sections and search
                    // jumps, which the stack's cannot, so the stack's goes.
                    .navigationDestination(for: AlbumRoute.self) { route in
                        AlbumDetailView(albumId: route.id)
                            .navigationBarBackButtonHidden(true)
                    }
                    .navigationDestination(for: ArtistRoute.self) { route in
                        ArtistDetailView(artistId: route.id)
                            .navigationBarBackButtonHidden(true)
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
        .onChange(of: search.query) { _, _ in search.schedule() }
        .onSubmit(of: .search) { handleSubmit() }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(spacing: 0) {
                Divider()
                TransportBar()
            }
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
        .toolbar {
            // Spans section switches and search jumps, which a NavigationStack's
            // own back button cannot — it only knows about one stack.
            ToolbarItemGroup(placement: .navigation) {
                Button { library.goBack() } label: {
                    Label("Back", systemImage: "chevron.left")
                }
                .disabled(!library.canGoBack)
                .help("Back (⌘[)")

                Button { library.goForward() } label: {
                    Label("Forward", systemImage: "chevron.right")
                }
                .disabled(!library.canGoForward)
                .help("Forward (⌘])")
            }

            // Separate items, not one ToolbarItemGroup: a group is drawn as a
            // single joined control, which put the filter field and the lyrics
            // toggle inside the same capsule.
            ToolbarItem(placement: .primaryAction) {
                Spacer()
            }

            // Filtering what is on screen belongs with it, not in the sidebar
            // search, which navigates away instead of narrowing.
            if library.section == .albums || library.section == .artists {
                ToolbarItem(placement: .primaryAction) {
                    FilterField(
                        placeholder: library.section == .albums
                            ? "Filter albums" : "Filter artists",
                        text: $library.filter
                    )
                    .frame(width: 180)
                }
            }

            // Sort belongs next to what it sorts, so it only appears there.
            if library.section == .albums {
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

                        if library.albumSort == .random {
                            Divider()
                            Button("Shuffle Again") { library.reshuffleAlbums() }
                        }
                    } label: {
                        Label("Sort", systemImage: "arrow.up.arrow.down")
                    }
                    // The accent marks what is playing and what is selected.
                    // A toolbar control that is always there is neither.
                    .tint(.primary)
                    .help("Sort albums — \(library.albumSort.label)")
                }
            }

        }
        .sheet(isPresented: $showingPicker) {
            PickerSheet(isPresented: $showingPicker)
        }
        .overlay(alignment: .bottom) {
            if let error = player.lastError {
                ErrorToast(message: error) { player.lastError = nil }
                    .padding(.bottom, 24)
            }
        }
    }

    /// Return either picks a suggestion — in which case the field holds a token
    /// naming exactly what was chosen — or it means "show me everything".
    private func handleSubmit() {
        guard let selection = SearchModel.Selection(token: search.query) else {
            library.section = .searchResults
            return
        }
        switch selection {
        case .track(let id, let albumId):
            // A track lives on its album; that's where you'd play it from.
            if let albumId { library.reveal(album: albumId, highlighting: id) }
        case .album(let id):
            library.reveal(album: id)
        case .artist(let id):
            library.reveal(artist: id)
        }
        search.reset()
    }

    @ViewBuilder
    private var stage: some View {
        switch library.section {
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
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(.separator))
        .shadow(radius: 12, y: 4)
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
