import KoanFFI
import SwiftUI

struct AlbumBrowser: View {
    @Environment(LibraryModel.self) private var library
    @Environment(UIState.self) private var ui

    private let columns = [GridItem(.adaptive(minimum: 150, maximum: 210), spacing: 18)]

    var body: some View {
        ScrollView {
                if library.visibleAlbums.isEmpty {
                    EmptyState(
                        icon: "square.stack",
                        title: library.filter.isEmpty ? "No albums yet" : "Nothing matches",
                        detail: library.filter.isEmpty
                            ? "Run a scan to index your music folders."
                            : "Try a different filter."
                    )
                    .frame(maxWidth: .infinity, minHeight: 340)
                } else {
                    LazyVGrid(columns: columns, spacing: 22) {
                        ForEach(library.visibleAlbums, id: \.id) { album in
                            AlbumGridCell(album: album, selectable: true)
                        }
                    }
                    .padding(20)
                    // Dragging a ticked tile carries every tick, in the order
                    // they were made; an unticked one carries itself.
                    //
                    // Worked out here, at drag time, rather than handed to the
                    // container as its selection: that was a read of the ticks
                    // in the grid's body, and every tick re-diffed the grid.
                    // What it costs is the preview — a stack of ticks drags as
                    // the one tile under the pointer.
                    .dragContainer(for: PlayableTransfer.self, itemID: \.id) { grabbed in
                        let selection = library.selection
                        let ids = grabbed.contains(where: selection.contains)
                            ? selection.ids
                            : Array(grabbed)
                        return ids.map { id in
                            let name = library.visibleAlbums.first { $0.id == id }?.title ?? ""
                            return PlayableTransfer(kind: .album, id: id, name: name)
                        }
                    }
                }
            }
            // ⌘A picks everything the filter is showing, starting a selection
            // if there was none. Escape and leaving the page drop it.
            .onChange(of: ui.selectAllToken) { _, _ in
                library.selection.selectAll(library.visibleAlbums)
            }
            .onChange(of: ui.clearSelectionToken) { _, _ in library.selection.end() }
            .onDisappear { library.selection.end() }
    }
}

struct AlbumDetailView: View {
    let albumId: Int64

    @Environment(LibraryModel.self) private var library
    @Environment(PlayerModel.self) private var player

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
