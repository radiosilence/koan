import KoanFFI
import SwiftUI

struct AlbumBrowser: View {
    @Environment(LibraryModel.self) private var library
    @Environment(UIState.self) private var ui
    @Environment(Navigator.self) private var nav
    /// Where the grid is. Seeded from the model on the way in and written back
    /// on the way out, since the browser is rebuilt on each visit.
    @State private var position = ScrollPosition(idType: Int64.self)
    /// Kept mounted behind other pages once visited — see `StageView`.
    @Environment(\.onStage) private var onStage

    private let columns = [GridItem(.adaptive(minimum: 150, maximum: 210), spacing: 18)]
    #if os(macOS)
    private static let emptyDetail = "Add a music folder in Settings → Library, or sign in to a server in Settings → Server."
    #else
    private static let emptyDetail = "Sign in to your music server in Settings → Server."
    #endif

    var body: some View {
        ScrollView {
                if library.visibleAlbums.isEmpty {
                    EmptyState(
                        icon: "square.stack",
                        title: library.filter.isEmpty ? "No albums yet" : "Nothing matches",
                        detail: library.filter.isEmpty
                            ? Self.emptyDetail
                            : "Try a different filter."
                    )
                    .frame(maxWidth: .infinity, minHeight: 340)
                } else {
                    LazyVGrid(columns: columns, spacing: 22) {
                        ForEach(library.visibleAlbums, id: \.id) { album in
                            AlbumGridCell(album: album, selection: library.selection)
                        }
                    }
                    .scrollTargetLayout()
                    .padding(20)
                    .modifier(SelectionDrag(selection: library.selection))
                }
            }
            // ⌘A picks everything the filter is showing, starting a selection
            // if there was none — only while this is the page on screen, since
            // it stays mounted behind the others. Escape and leaving the page
            // drop it.
            .onChange(of: ui.selectAllToken) { _, _ in
                guard onStage else { return }
                library.selection.selectAll()
            }
            .onChange(of: ui.clearSelectionToken) { _, _ in library.selection.end() }
            .onChange(of: onStage) { _, now in if !now { library.selection.end() } }
            .onDisappear { library.selection.end() }
            // Rebuilt on each visit rather than kept mounted behind other pages
            // (see `StageView`), and put back where it was.
            .scrollPosition($position, anchor: .top)
            .onAppear {
                if let top = library.albumsTop { position.scrollTo(id: top, anchor: .top) }
            }
            .onDisappear { library.albumsTop = position.viewID(type: Int64.self) }
            .onChange(of: nav.rewinds[.albums]) { position.scrollTo(edge: .top) }
    }
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
