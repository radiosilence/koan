import SwiftUI

struct AlbumListView: View {
    @Environment(ServerConfig.self) private var config
    @State private var albums: [Album] = []
    @State private var searchText = ""
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
        .navigationTitle("Albums")
        .navigationDestination(for: Album.self) { album in
            AlbumDetailView(album: album)
        }
        .searchable(text: $searchText, prompt: "Search albums")
        .onChange(of: searchText) { _, _ in
            Task { await reload() }
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
            "first": pageSize,
            "favouritesOnly": false,
        ]
        if let endCursor { variables["after"] = endCursor }
        let trimmed = searchText.trimmingCharacters(in: .whitespaces)
        if !trimmed.isEmpty { variables["search"] = trimmed }

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
