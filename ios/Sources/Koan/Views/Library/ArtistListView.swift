import SwiftUI

struct ArtistListView: View {
    @State private var artists: [Artist] = []
    @State private var isLoading = false
    @State private var error: String?
    @State private var searchText = ""

    private var client: GraphQLClient { .shared }

    var body: some View {
        Group {
            if isLoading && artists.isEmpty {
                ProgressView("Loading artists...")
            } else if let error {
                ContentUnavailableView {
                    Label("Connection Failed", systemImage: "wifi.slash")
                } description: {
                    Text(error)
                } actions: {
                    Button("Retry") { Task { await loadArtists() } }
                }
            } else if artists.isEmpty {
                ContentUnavailableView("No Artists", systemImage: "music.mic",
                                       description: Text("Library is empty or server unreachable."))
            } else {
                List(artists) { artist in
                    NavigationLink(value: artist) {
                        VStack(alignment: .leading) {
                            Text(artist.name)
                                .font(.body)
                            if let albums = artist.albumCount, let tracks = artist.trackCount {
                                Text("\(albums) albums, \(tracks) tracks")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                        }
                    }
                }
            }
        }
        .navigationTitle("Artists")
        .navigationDestination(for: Artist.self) { artist in
            AlbumListView(artistId: artist.id, artistName: artist.name)
        }
        .searchable(text: $searchText, prompt: "Search artists")
        .onChange(of: searchText) { _, _ in
            Task { await loadArtists() }
        }
        .refreshable { await loadArtists() }
        .task { await loadArtists() }
    }

    private func loadArtists() async {
        isLoading = true
        defer { isLoading = false }

        var variables: [String: Any] = [:]
        if !searchText.isEmpty { variables["search"] = searchText }

        do {
            let response: ArtistsResponse = try await client.execute(
                query: GQL.artists, variables: variables.isEmpty ? nil : variables)
            artists = response.artists.nodes
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }
}

extension Artist: Hashable {
    static func == (lhs: Artist, rhs: Artist) -> Bool { lhs.id == rhs.id }
    func hash(into hasher: inout Hasher) { hasher.combine(id) }
}
