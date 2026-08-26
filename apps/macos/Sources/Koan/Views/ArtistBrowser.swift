import KoanFFI
import SwiftUI

struct ArtistBrowser: View {
    @Environment(LibraryModel.self) private var library
    @Environment(Navigator.self) private var nav
    /// Without a selection binding a List row has nothing to do with a click —
    /// which is why this list felt completely dead.
    @State private var selection: Set<Int64> = []

    var body: some View {
        List(library.visibleArtists, id: \.id, selection: $selection) { artist in
            ArtistRow(artist: artist)
        }
        .clearsSelection($selection)
        .washedGround()
        .contextMenu(forSelectionType: Int64.self) { ids in
            if let id = ids.first,
               let artist = library.visibleArtists.first(where: { $0.id == id }) {
                PlayableMenu(playable: .artist(id: artist.id, name: artist.name))
            }
        } primaryAction: { ids in
            if let id = ids.first { nav.open(artist: id) }
        }
        .overlay {
            if library.visibleArtists.isEmpty {
                EmptyState(icon: "music.mic", title: "No artists yet")
            }
        }
    }
}

/// One artist.
///
/// Its own view so that hovering it invalidates one row. With the hover state
/// on the browser, every pointer move across the list rebuilt all of it —
/// thousands of rows diffed to light up a play button.
private struct ArtistRow: View {
    let artist: Artist

    @Environment(LibraryModel.self) private var library
    @State private var hovered = false

    var body: some View {
        HStack(spacing: 10) {
            Group {
                if hovered {
                    RowPlayButton(playable: playable, visible: true)
                } else {
                    Image(systemName: "music.mic")
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                }
            }
            .frame(width: 18, height: 18)
            // The name is the way in — a link, so a single click opens the
            // artist while the rest of the row selects.
            LinkText(
                text: artist.name,
                target: .artist(artist.id),
                font: .body,
                prominent: true
            )
            FavouriteButton(
                isOn: library.isFavourite(artist: artist.id),
                showing: hovered,
                size: .caption
            ) {
                library.toggleFavourite(artist: artist.id)
            }
            .frame(width: 16)
            Spacer(minLength: 12)
            Text(Format.count(artist.albumCount, "album"))
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: 78, alignment: .trailing)
            Text(Format.count(artist.trackCount, "track"))
                .font(.caption.monospacedDigit())
                .foregroundStyle(.tertiary)
                .frame(width: 78, alignment: .trailing)
        }
        .onHover { hovered = $0 }
        .frame(height: 24)
        .rowBehaviour(playable: playable)
    }

    private var playable: Playable { .artist(id: artist.id, name: artist.name) }
}

/// An artist's records as a grid, since that's how people think about a
/// discography — the flat track list is a click away on each album.
struct ArtistDetailView: View {
    let artistId: Int64

    @Environment(LibraryModel.self) private var library
    @Environment(Navigator.self) private var nav
    @Environment(PlayerModel.self) private var player

    /// Fetched, not looked up: the browser holds the page it is showing, and
    /// this artist may well not be on it.
    @State private var artist: Artist?
    @State private var albums: [Album] = []
    @State private var similar: [SimilarArtist] = []

    private let columns = [GridItem(.adaptive(minimum: 150, maximum: 210), spacing: 18)]

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                // The play button reads as part of the title, so it sits on the
                // title's line. Everything below is full width rather than
                // indented into a column beside it.
                VStack(alignment: .leading, spacing: 6) {
                    HStack(alignment: .firstTextBaseline, spacing: 14) {
                        if let artist {
                            PlayableHeaderButton(
                                playable: .artist(id: artist.id, name: artist.name)
                            )
                            .alignmentGuide(.firstTextBaseline) { $0[.bottom] * 0.78 }
                        }
                        Text(artist?.name ?? "Artist")
                            .font(.system(size: 26, weight: .semibold))
                    }
                    Text(Format.count(Int64(albums.count), "album"))
                        .font(.callout)
                        .foregroundStyle(.secondary)
                    if let artist {
                        let playable = Playable.artist(id: artist.id, name: artist.name)
                        HStack(spacing: 10) {
                            QueueButtons(playable: playable)
                            ShareButton(playable: playable)
                            FavouriteHeaderButton(playable: playable)
                        }
                        .padding(.top, 4)
                    }
                    Spacer()
                    Button {
                        shufflePlay()
                    } label: {
                        Label("Shuffle", systemImage: Icon.shuffle)
                    }
                }

                LazyVGrid(columns: columns, spacing: 22) {
                    ForEach(albums, id: \.id) { album in
                        AlbumGridCell(album: album, showArtist: false)
                    }
                }

                if !similar.isEmpty {
                    Divider()
                    Text("Similar Artists")
                        .font(.headline)
                    // Cached relationships only — this never reaches the network.
                    FlowLayout(spacing: 8) {
                        ForEach(similar, id: \.artistId) { entry in
                            ArtistPill(name: entry.name, artistId: entry.artistId)
                        }
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
        artist = try? await engine.artist(artistId: id)
        albums = (try? await engine.albums(artistId: id, sort: .year, seed: 0, search: nil)) ?? []
        similar = (try? await engine.similarArtists(artistId: id)) ?? []
    }

    private func shufflePlay() {
        let engine = library.engine
        let id = artistId
        Task {
            let ids = ((try? await engine.randomTracks(count: 50, artistId: id)) ?? []).map(\.id)
            player.playNow(trackIds: ids)
            nav.showQueueWhenReady(watching: player)
        }
    }
}

/// Wrapping row of chips. SwiftUI still has no built-in flow layout.
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
