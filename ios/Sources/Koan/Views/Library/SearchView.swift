import SwiftUI

struct SearchView: View {
    @Environment(ServerConfig.self) private var config
    @State private var searchText = ""
    @State private var artists: [Artist] = []
    @State private var albums: [Album] = []
    @State private var tracks: [Track] = []
    @State private var isLoading = false
    @State private var error: String?
    @State private var searchTask: Task<Void, Never>?

    var body: some View {
        List {
            if !artists.isEmpty {
                Section("Artists") {
                    ForEach(artists) { artist in
                        NavigationLink {
                            ArtistDetailView(artist: artist)
                        } label: {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(artist.name)
                                Text("\(artist.albumCount) albums")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                        }
                    }
                }
            }

            if !albums.isEmpty {
                Section("Albums") {
                    ForEach(albums) { album in
                        NavigationLink {
                            AlbumDetailView(album: album)
                        } label: {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(album.title)
                                HStack(spacing: 4) {
                                    Text(album.artistName)
                                    if let year = album.year {
                                        Text("\u{00B7} \(year)")
                                    }
                                }
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            }
                        }
                    }
                }
            }

            if !tracks.isEmpty {
                Section("Tracks") {
                    ForEach(tracks) { track in
                        TrackRowView(track: track) {
                            await config.client.addToQueue(trackIds: [track.id])
                        } toggleFavourite: {
                            await config.client.toggleFavourite(trackId: track.id)
                            let trimmed = searchText.trimmingCharacters(in: .whitespaces)
                            if !trimmed.isEmpty { await search(query: trimmed) }
                        }
                    }
                }
            }

            if artists.isEmpty && albums.isEmpty && tracks.isEmpty && !searchText.isEmpty && !isLoading {
                ContentUnavailableView.search(text: searchText)
            }

            if let error {
                Text(error)
                    .foregroundStyle(.red)
                    .font(.caption)
            }
        }
        .navigationTitle("Search")
        .searchable(text: $searchText, isPresented: .constant(true), prompt: "Artists, albums, tracks")
        .onChange(of: searchText) { _, newValue in
            searchTask?.cancel()
            let trimmed = newValue.trimmingCharacters(in: .whitespaces)
            guard !trimmed.isEmpty else {
                artists = []
                albums = []
                tracks = []
                return
            }
            searchTask = Task {
                try? await Task.sleep(for: .milliseconds(300))
                guard !Task.isCancelled else { return }
                await search(query: trimmed)
            }
        }
    }

    private func search(query: String) async {
        isLoading = true
        defer { isLoading = false }

        let vars: [String: Any] = ["search": query, "first": 10, "favouritesOnly": false]

        async let artistsReq: ArtistsResponse = config.client.execute(
            query: GQL.artists, variables: vars
        )
        async let albumsReq: AlbumsResponse = config.client.execute(
            query: GQL.albums, variables: vars
        )
        async let tracksReq: TracksResponse = config.client.execute(
            query: GQL.tracks, variables: vars
        )

        do {
            let (a, al, t) = try await (artistsReq, albumsReq, tracksReq)
            artists = a.artists.nodes
            albums = al.albums.nodes
            tracks = t.tracks.nodes
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }

}
