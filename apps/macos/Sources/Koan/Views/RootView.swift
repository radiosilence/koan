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

    var body: some View {
        @Bindable var library = library
        @Bindable var search = search

        NavigationSplitView {
            SidebarView()
                .navigationSplitViewColumnWidth(min: 190, ideal: 215, max: 290)
        } detail: {
            NavigationStack(path: $library.path) {
                stage
                    // The transport sits over the bottom of the detail column,
                    // and without this the scrollbar runs underneath it and is
                    // impossible to grab near the end of a long list.
                    .contentMargins(.bottom, 64, for: .scrollIndicators)
                    .navigationDestination(for: AlbumRoute.self) { route in
                        AlbumDetailView(albumId: route.id)
                    }
                    .navigationDestination(for: ArtistRoute.self) { route in
                        ArtistDetailView(artistId: route.id)
                    }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .inspector(isPresented: $showLyrics) {
            LyricsPanel()
                .inspectorColumnWidth(min: 260, ideal: 320, max: 460)
        }
        .onChange(of: search.query) { _, _ in search.schedule() }
        .onSubmit(of: .search) { handleSubmit() }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(spacing: 0) {
                Divider()
                TransportBar()
            }
        }
        .toolbar {
            // Sort belongs next to what it sorts, so it only appears there.
            if library.section == .albums {
                ToolbarItem(placement: .principal) {
                    Picker("Sort", selection: Binding(
                        get: { library.albumSort },
                        set: { library.albumSort = $0 }
                    )) {
                        ForEach(AlbumSort.all, id: \.self) { sort in
                            Text(sort.label).tag(sort)
                        }
                    }
                    .pickerStyle(.menu)
                    .labelsHidden()
                    .frame(width: 150)
                    .help("Sort albums")
                }
            }
            // Alone in the trailing slot, so it stays pinned to the right edge
            // whatever else the section puts in the toolbar — the mirror of the
            // sidebar toggle on the left.
            ToolbarItem(placement: .primaryAction) {
                Button {
                    showLyrics.toggle()
                } label: {
                    Label("Lyrics", systemImage: "sidebar.right")
                }
                .help("Lyrics panel (⌥⌘L)")
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
