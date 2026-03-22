import SwiftUI

struct ArtistDetailView: View {
    @Environment(ServerConfig.self) private var config
    let artist: Artist

    @State private var albums: [Album] = []
    @State private var isLoading = false
    @State private var hasNextPage = false
    @State private var endCursor: String?
    @State private var error: String?

    private let pageSize = 50
    private let columns = [
        GridItem(.adaptive(minimum: 160), spacing: 16)
    ]

    var body: some View {
        ScrollView {
            LazyVGrid(columns: columns, spacing: 16) {
                ForEach(albums) { album in
                    NavigationLink(value: album) {
                        AlbumCard(album: album, serverBaseURL: config.baseURL)
                    }
                    .buttonStyle(.plain)
                }

                if hasNextPage {
                    ProgressView()
                        .frame(maxWidth: .infinity)
                        .task { await loadMore() }
                }
            }
            .padding()

            if let error {
                Text(error)
                    .foregroundStyle(.red)
                    .font(.caption)
                    .padding()
            }
        }
        .navigationTitle(artist.name)
        .navigationDestination(for: Album.self) { album in
            AlbumDetailView(album: album)
        }
        .refreshable { await reload() }
        .task { await reload() }
    }

    private func reload() async {
        albums = []
        endCursor = nil
        hasNextPage = false
        await loadMore()
    }

    private func loadMore() async {
        guard !isLoading else { return }
        isLoading = true
        defer { isLoading = false }

        var variables: [String: Any] = [
            "artistId": artist.id,
            "first": pageSize,
            "favouritesOnly": false,
        ]
        if let endCursor { variables["after"] = endCursor }

        do {
            let response: AlbumsResponse = try await config.client.execute(
                query: GQL.albums, variables: variables
            )
            albums.append(contentsOf: response.albums.nodes)
            hasNextPage = response.albums.pageInfo.hasNextPage
            endCursor = response.albums.endCursor
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }
}

struct AlbumCard: View {
    let album: Album
    let serverBaseURL: URL

    private var coverURL: URL? {
        // Use first track of album for cover art — server serves cover at /cover/:trackId
        // We don't have a track ID here, so use album ID as a convention
        // The actual endpoint may need adjustment based on server implementation
        URL(string: "\(serverBaseURL)/cover/album/\(album.id)")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            AsyncImage(url: coverURL) { phase in
                switch phase {
                case .success(let image):
                    image
                        .resizable()
                        .aspectRatio(1, contentMode: .fill)
                case .failure:
                    placeholder
                default:
                    placeholder
                }
            }
            .frame(minHeight: 140)
            .clipShape(RoundedRectangle(cornerRadius: 8))

            Text(album.title)
                .font(.callout)
                .fontWeight(.medium)
                .lineLimit(2)

            HStack(spacing: 4) {
                if let year = album.year {
                    Text(year)
                }
                if let codec = album.codec {
                    Text("\u{00B7} \(codec)")
                }
            }
            .font(.caption)
            .foregroundStyle(.secondary)
        }
    }

    private var placeholder: some View {
        Rectangle()
            .fill(.quaternary)
            .aspectRatio(1, contentMode: .fill)
            .overlay {
                Image(systemName: "music.note")
                    .font(.title)
                    .foregroundStyle(.tertiary)
            }
    }
}
