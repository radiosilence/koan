import SwiftUI

struct AlbumListView: View {
    let artistId: Int
    let artistName: String

    @State private var albums: [Album] = []
    @State private var isLoading = false
    @State private var error: String?

    private var client: GraphQLClient { .shared }

    var body: some View {
        Group {
            if isLoading && albums.isEmpty {
                ProgressView("Loading albums...")
            } else if let error {
                ContentUnavailableView {
                    Label("Error", systemImage: "exclamationmark.triangle")
                } description: {
                    Text(error)
                } actions: {
                    Button("Retry") { Task { await loadAlbums() } }
                }
            } else if albums.isEmpty {
                ContentUnavailableView("No Albums", systemImage: "square.stack",
                                       description: Text("No albums found for this artist."))
            } else {
                List(albums) { album in
                    NavigationLink(value: album) {
                        VStack(alignment: .leading) {
                            Text(album.title)
                                .font(.body)
                            HStack(spacing: 8) {
                                if let date = album.date {
                                    Text(date)
                                }
                                if let codec = album.codec {
                                    Text(codec)
                                        .textCase(.uppercase)
                                }
                                if let count = album.trackCount {
                                    Text("\(count) tracks")
                                }
                            }
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        }
                    }
                }
            }
        }
        .navigationTitle(artistName)
        .navigationDestination(for: Album.self) { album in
            TrackListView(albumId: album.id, albumTitle: album.title)
        }
        .refreshable { await loadAlbums() }
        .task { await loadAlbums() }
    }

    private func loadAlbums() async {
        isLoading = true
        defer { isLoading = false }

        do {
            let response: AlbumsResponse = try await client.execute(
                query: GQL.albumsForArtist,
                variables: ["artistId": artistId])
            albums = response.albums.nodes
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }
}

extension Album: Hashable {
    static func == (lhs: Album, rhs: Album) -> Bool { lhs.id == rhs.id }
    func hash(into hasher: inout Hasher) { hasher.combine(id) }
}
