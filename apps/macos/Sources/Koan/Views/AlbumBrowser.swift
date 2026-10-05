import KoanFFI
import SwiftUI

struct AlbumBrowser: View {
    @Environment(LibraryModel.self) private var library
    @Environment(UIState.self) private var ui
    @Environment(Navigator.self) private var nav
    #if os(macOS)
    @Environment(PlayerModel.self) private var player
    @Environment(\.roomTint) private var tint
    @AppStorage("graphics") private var graphics = Graphics.full
    /// The toolbar's and the transport's share of the page, which the grid
    /// scrolls under.
    @State private var insets = EdgeInsets()
    #else
    /// Where the grid is scrolled to, as a distance rather than an album: a
    /// reshuffle reorders every album, and following the one at the top would
    /// carry the grid to wherever it landed.
    @State private var position = ScrollPosition()

    private let columns = GridItem.tiles(minimum: 150, maximum: 210, spacing: 18)
    #endif

    #if os(macOS)
    private static let emptyDetail = "Add a music folder in Settings → Library, or sign in to a server in Settings → Server."
    #else
    private static let emptyDetail = "Sign in to your music server in Settings → Server."
    #endif

    var body: some View {
        albums
            // ⌘A picks everything the filter is showing, starting a selection
            // if there was none. Escape and leaving the page drop it.
            .onChange(of: ui.selectAllToken) { _, _ in library.selection.selectAll() }
            .onChange(of: ui.clearSelectionToken) { _, _ in library.selection.end() }
            .onDisappear { library.selection.end() }
    }

    private var empty: some View {
        EmptyState(
            icon: "square.stack",
            title: library.isNarrowed ? "Nothing matches" : "No albums yet",
            detail: !library.isNarrowed
                ? Self.emptyDetail
                : "Try a different filter."
        )
        .frame(maxWidth: .infinity, minHeight: 340)
    }

    #if os(macOS)
    /// An `AlbumCollection` — see there for why the Mac's grid is AppKit.
    @ViewBuilder
    private var albums: some View {
        if library.visibleAlbums.isEmpty {
            ScrollView { empty }
        } else {
            let selection = library.selection
            AlbumCollection(
                albums: library.visibleAlbums,
                selection: selection,
                picked: Set(selection.picked.map(\.key)),
                selecting: selection.isActive,
                favourites: library.favouriteAlbumIds,
                tint: tint,
                usesGlass: graphics.usesGlass,
                insets: insets,
                rewinds: nav.rewinds[.albums] ?? 0,
                actions: actions
            )
            .ignoresSafeArea()
            .background {
                Color.clear.onGeometryChange(for: EdgeInsets.self) { $0.safeAreaInsets } action: { insets = $0 }
            }
        }
    }

    private var actions: AlbumTile.Actions {
        let library = library
        let nav = nav
        let player = player
        return AlbumTile.Actions(
            open: { nav.open(album: $0) },
            openArtist: { nav.open(artist: $0) },
            // The record is where the tracks are, so that is where a click on
            // its sleeve leaves you, as `PlayableArtwork` does.
            play: { id in
                nav.open(album: id)
                let engine = library.engine
                let ids = await Task.detached { (try? await engine.trackIds(albumId: id, artistId: nil)) ?? [] }.value
                player.playNow(trackIds: ids)
            },
            toggleFavourite: { library.toggleFavourite(album: $0) }
        )
    }
    #else
    private var albums: some View {
        ScrollView {
            if library.visibleAlbums.isEmpty {
                empty
            } else {
                LazyVGrid(columns: columns, spacing: 22) {
                    ForEach(library.visibleAlbums, id: \.id) { album in
                        AlbumGridCell(album: album, selection: library.selection)
                    }
                }
                .padding(20)
                .modifier(SelectionDrag(selection: library.selection))
            }
        }
        // Rebuilt on each visit rather than kept mounted behind other pages
        // (see `StageView`), and put back where it was.
        .scrollPosition($position)
        .onScrollGeometryChange(for: CGFloat.self) { $0.contentOffset.y } action: { _, y in
            library.albumsOffset = y
        }
        .onAppear {
            if let y = library.albumsOffset { position.scrollTo(y: y) }
        }
        .onChange(of: nav.rewinds[.albums]) { position.scrollTo(edge: .top) }
    }
    #endif
}

struct AlbumDetailView: View {
    let albumId: Int64

    @Environment(LibraryModel.self) private var library

    /// Whatever the navigator loaded before it brought us here, so the first
    /// body evaluation already has the whole page. Guarded on the id because
    /// history can move faster than a read.
    private var record: LibraryModel.AlbumRecord? {
        let held = library.detailRecord
        return held?.albumId == albumId ? held : nil
    }

    var body: some View {
        Trace.event("album-body")
        FrameTimer.shared.evaluated()
        return TrackListView(
            title: record?.album?.title ?? "Album",
            subtitle: subtitle,
            tracks: record?.tracks ?? [],
            artwork: .album(albumId),
            artistLink: record?.album?.artistId,
            playable: record?.album.map { Playable.album($0) }
        )
        // Only for a library change — the record itself arrived before the page
        // did. A download landing writes a cached path onto one of these rows.
        .reloading(on: albumId) { await library.prepare(album: albumId) }
    }

    private var subtitle: String {
        guard let record, let album = record.album else { return "" }
        var parts = [album.artistName]
        if let year = album.year { parts.append(String(year)) }
        if let codec = album.codec { parts.append(codec.uppercased()) }
        let total = record.tracks.compactMap(\.durationMs).reduce(0, +)
        if total > 0 {
            parts.append(Format.duration(total))
        }
        return parts.joined(separator: " · ")
    }
}

struct EmptyState: View {
    let icon: String
    let title: String
    var detail: String?

    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: icon)
                .font(.system(size: 32, weight: .light))
                .foregroundStyle(.tertiary)
            Text(title)
                .font(.title3)
                .foregroundStyle(.secondary)
            if let detail {
                Text(detail)
                    .font(.callout)
                    .foregroundStyle(.tertiary)
            }
        }
    }
}

/// The page as a drag container, so that dragging a ticked item carries every
/// tick in the order they were made, and an unticked one carries itself.
///
/// Worked out at drag time rather than handed to the container as its
/// selection: that would be a read of the ticks in the grid's body, and every
/// tick would re-diff the grid. What it costs is the preview — a stack of ticks drags as
/// the one item under the pointer.
struct SelectionDrag: ViewModifier {
    let selection: PlayableSelection

    func body(content: Content) -> some View {
        #if os(macOS)
        content.dragContainer(for: PlayableTransfer.self, itemID: \.key) { grabbed in
            let items = grabbed.contains(where: selection.contains)
                ? selection.picked
                : selection.grid().filter { grabbed.contains($0.key) }
            return items.map(PlayableTransfer.init)
        }
        #else
        content
        #endif
    }
}
