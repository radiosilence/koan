import KoanFFI
import SwiftUI

struct AlbumBrowser: View {
    @Environment(LibraryModel.self) private var library

    private let columns = [GridItem(.adaptive(minimum: 150, maximum: 210), spacing: 18)]

    var body: some View {
        NavigationStack {
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
                            NavigationLink(value: AlbumRoute(id: album.id)) {
                                AlbumCell(album: album)
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    .padding(20)
                }
            }
            .navigationDestination(for: AlbumRoute.self) { route in
                AlbumDetailView(albumId: route.id)
            }
        }
    }
}

private struct AlbumCell: View {
    let album: Album

    @Environment(PlayerModel.self) private var player
    @Environment(LibraryModel.self) private var library

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            AlbumArtwork(source: .album(album.id))
                .shadow(color: .black.opacity(0.28), radius: 7, y: 3)

            Text(album.title)
                .font(.callout.weight(.medium))
                .lineLimit(1)
            HStack(spacing: 4) {
                Text(album.artistName)
                    .lineLimit(1)
                if let year = album.year {
                    Text("· \(String(year))")
                }
            }
            .font(.caption)
            .foregroundStyle(.secondary)
        }
        .contentShape(.rect)
        .contextMenu {
            Button("Play") { play(replacing: true) }
            Button("Add to Queue") { play(replacing: false) }
        }
    }

    /// Loading the tracks is the slow half, so it happens off the main actor and
    /// the queue command follows once they're in hand.
    private func play(replacing: Bool) {
        let engine = library.engine
        let albumId = album.id
        Task {
            let ids = await Task.detached(priority: .userInitiated) {
                ((try? engine.tracks(
                    albumId: albumId, artistId: nil, sort: .album, limit: 500, offset: 0
                )) ?? []).map(\.id)
            }.value
            if replacing { player.playNow(trackIds: ids) } else { player.enqueue(trackIds: ids) }
        }
    }
}

struct AlbumDetailView: View {
    let albumId: Int64

    @Environment(LibraryModel.self) private var library
    @Environment(PlayerModel.self) private var player

    private var album: Album? { library.albums.first { $0.id == albumId } }

    var body: some View {
        TrackListView(
            title: album?.title ?? "Album",
            subtitle: subtitle,
            tracks: library.detailTracks,
            artwork: .album(albumId)
        )
        .task(id: albumId) {
            library.loadTracks(albumId: albumId)
        }
    }

    private var subtitle: String {
        guard let album else { return "" }
        var parts = [album.artistName]
        if let year = album.year { parts.append(String(year)) }
        if let codec = album.codec { parts.append(codec.uppercased()) }
        let total = library.detailTracks.compactMap(\.durationMs).reduce(0, +)
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
