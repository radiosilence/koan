import KoanFFI
import SwiftUI

struct ArtistBrowser: View {
    @Environment(LibraryModel.self) private var library

    var body: some View {
        List(library.visibleArtists, id: \.id) { artist in
            NavigationLink(value: ArtistRoute(id: artist.id)) {
                HStack(spacing: 10) {
                    Image(systemName: "music.mic")
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                        .frame(width: 18)
                    Text(artist.name)
                }
                .padding(.vertical, 2)
            }
        }
        .overlay {
            if library.visibleArtists.isEmpty {
                EmptyState(icon: "music.mic", title: "No artists yet")
            }
        }
    }
}

/// An artist's records as a grid, since that's how people think about a
/// discography — the flat track list is a click away on each album.
struct ArtistDetailView: View {
    let artistId: Int64

    @Environment(LibraryModel.self) private var library
    @Environment(PlayerModel.self) private var player

    @State private var albums: [Album] = []
    @State private var similar: [SimilarArtist] = []

    private let columns = [GridItem(.adaptive(minimum: 150, maximum: 210), spacing: 18)]

    private var artist: Artist? { library.artists.first { $0.id == artistId } }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                HStack(alignment: .center, spacing: 14) {
                    VStack(alignment: .leading, spacing: 4) {
                        Text(artist?.name ?? "Artist")
                            .font(.system(size: 26, weight: .semibold))
                        Text(Format.count(Int64(albums.count), "album"))
                            .font(.callout)
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                    Button {
                        shufflePlay()
                    } label: {
                        Label("Shuffle", systemImage: "shuffle")
                    }
                }

                LazyVGrid(columns: columns, spacing: 22) {
                    ForEach(albums, id: \.id) { album in
                        NavigationLink(value: AlbumRoute(id: album.id)) {
                            VStack(alignment: .leading, spacing: 7) {
                                AlbumArtwork(source: .album(album.id))
                                    .shadow(color: .black.opacity(0.28), radius: 7, y: 3)
                                Text(album.title)
                                    .font(.callout.weight(.medium))
                                    .lineLimit(1)
                                if let year = album.year {
                                    Text(String(year))
                                        .font(.caption)
                                        .foregroundStyle(.secondary)
                                }
                            }
                        }
                        .buttonStyle(.plain)
                    }
                }

                if !similar.isEmpty {
                    Divider()
                    Text("Similar Artists")
                        .font(.headline)
                    // Cached relationships only — this never reaches the network.
                    FlowRow(items: similar) { entry in
                        Text(entry.name)
                            .font(.callout)
                            .padding(.horizontal, 10)
                            .padding(.vertical, 5)
                            .background(.quaternary, in: Capsule())
                    }
                }
            }
            .padding(22)
        }
        .task(id: artistId) { await load() }
    }

    private func load() async {
        let engine = library.engine
        let id = artistId
        albums = await Task.detached(priority: .userInitiated) {
            (try? engine.albums(artistId: id, sort: .year)) ?? []
        }.value
        similar = await Task.detached(priority: .utility) {
            (try? engine.similarArtists(artistId: id)) ?? []
        }.value
    }

    private func shufflePlay() {
        let engine = library.engine
        let id = artistId
        Task {
            let ids = await Task.detached(priority: .userInitiated) {
                ((try? engine.randomTracks(count: 50, artistId: id)) ?? []).map(\.id)
            }.value
            player.playNow(trackIds: ids)
        }
    }
}

/// Wrapping row of chips. SwiftUI has no built-in flow layout on macOS 14.
struct FlowRow<Item: Identifiable, Content: View>: View {
    let items: [Item]
    @ViewBuilder let content: (Item) -> Content

    var body: some View {
        FlowLayout(spacing: 8) {
            ForEach(items) { content($0) }
        }
    }
}

extension SimilarArtist: Identifiable {
    public var id: Int64 { artistId }
}

struct FlowLayout: Layout {
    var spacing: CGFloat = 8

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width ?? 400
        var x: CGFloat = 0, y: CGFloat = 0, rowHeight: CGFloat = 0
        for view in subviews {
            let size = view.sizeThatFits(.unspecified)
            if x + size.width > width, x > 0 {
                x = 0
                y += rowHeight + spacing
                rowHeight = 0
            }
            x += size.width + spacing
            rowHeight = max(rowHeight, size.height)
        }
        return CGSize(width: width, height: y + rowHeight)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var x = bounds.minX, y = bounds.minY, rowHeight: CGFloat = 0
        for view in subviews {
            let size = view.sizeThatFits(.unspecified)
            if x + size.width > bounds.maxX, x > bounds.minX {
                x = bounds.minX
                y += rowHeight + spacing
                rowHeight = 0
            }
            view.place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(size))
            x += size.width + spacing
            rowHeight = max(rowHeight, size.height)
        }
    }
}
