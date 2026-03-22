import SwiftUI

struct FavouritesView: View {
    @Environment(ServerConfig.self) private var config
    @State private var tracks: [Track] = []
    @State private var isLoading = false
    @State private var hasNextPage = false
    @State private var endCursor: String?
    @State private var error: String?

    private let pageSize = 50

    var body: some View {
        List {
            if tracks.isEmpty && !isLoading && error == nil {
                ContentUnavailableView(
                    "No Favourites",
                    systemImage: "heart.slash",
                    description: Text("Long-press a track to add it to your favourites.")
                )
            }

            ForEach(tracks) { track in
                TrackRowView(track: track) {
                    await config.client.addToQueue(trackIds: [track.id])
                } toggleFavourite: {
                    await config.client.toggleFavourite(trackId: track.id)
                    await reload()
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
        .navigationTitle("Favourites")
        .refreshable { await reload() }
        .task { await reload() }
    }

    private func reload() async {
        tracks = []
        endCursor = nil
        hasNextPage = false
        await loadMore()
    }

    private func loadMore() async {
        guard !isLoading else { return }
        isLoading = true
        defer { isLoading = false }

        var variables: [String: Any] = ["first": pageSize]
        if let endCursor { variables["after"] = endCursor }

        do {
            let response: FavouritesResponse = try await config.client.execute(
                query: GQL.favourites, variables: variables
            )
            tracks.append(contentsOf: response.favourites.nodes)
            hasNextPage = response.favourites.pageInfo.hasNextPage
            endCursor = response.favourites.endCursor
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }

}
