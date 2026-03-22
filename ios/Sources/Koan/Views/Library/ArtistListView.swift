import SwiftUI

struct ArtistListView: View {
    @Environment(ServerConfig.self) private var config
    @State private var artists: [Artist] = []
    @State private var searchText = ""
    @State private var isLoading = false
    @State private var hasNextPage = false
    @State private var endCursor: String?
    @State private var error: String?

    private let pageSize = 50

    var body: some View {
        List {
            ForEach(artists) { artist in
                NavigationLink(value: artist) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(artist.name)
                            .font(.body)
                        Text("\(artist.albumCount) albums \u{00B7} \(artist.trackCount) tracks")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
            }

            if hasNextPage {
                ProgressView()
                    .frame(maxWidth: .infinity)
                    .task { await loadMore() }
            }

            if let error {
                Text(error)
                    .foregroundStyle(.red)
                    .font(.caption)
            }
        }
        .navigationTitle("Artists")
        .navigationDestination(for: Artist.self) { artist in
            ArtistDetailView(artist: artist)
        }
        .searchable(text: $searchText, prompt: "Search artists")
        .onChange(of: searchText) { _, _ in
            Task { await reload() }
        }
        .refreshable { await reload() }
        .task { await reload() }
    }

    private func reload() async {
        artists = []
        endCursor = nil
        hasNextPage = false
        await loadMore()
    }

    private func loadMore() async {
        guard !isLoading else { return }
        isLoading = true
        defer { isLoading = false }

        var variables: [String: Any] = [
            "first": pageSize,
            "favouritesOnly": false,
        ]
        if let endCursor { variables["after"] = endCursor }
        let trimmed = searchText.trimmingCharacters(in: .whitespaces)
        if !trimmed.isEmpty { variables["search"] = trimmed }

        do {
            let response: ArtistsResponse = try await config.client.execute(
                query: GQL.artists, variables: variables
            )
            artists.append(contentsOf: response.artists.nodes)
            hasNextPage = response.artists.pageInfo.hasNextPage
            endCursor = response.artists.endCursor
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }
}
