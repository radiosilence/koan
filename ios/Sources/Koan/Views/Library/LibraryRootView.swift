import SwiftUI

struct LibraryRootView: View {
    @Environment(ServerConfig.self) private var config
    @State private var stats: LibraryStats?
    @State private var error: String?
    @State private var isLoading = false

    var body: some View {
        List {
            if let stats {
                Section("Library") {
                    StatRow(label: "Artists", value: "\(stats.totalArtists)")
                    StatRow(label: "Albums", value: "\(stats.totalAlbums)")
                    StatRow(label: "Tracks", value: "\(stats.totalTracks)")
                }

                Section("Sources") {
                    StatRow(label: "Local", value: "\(stats.localTracks)")
                    StatRow(label: "Remote", value: "\(stats.remoteTracks)")
                    StatRow(label: "Cached", value: "\(stats.cachedTracks)")
                }
            }

            Section("Browse") {
                NavigationLink {
                    ArtistListView()
                } label: {
                    Label("Artists", systemImage: "person.2")
                }
                NavigationLink {
                    AlbumListView()
                } label: {
                    Label("Albums", systemImage: "square.stack")
                }
            }

            if let error {
                Section {
                    Text(error)
                        .foregroundStyle(.red)
                        .font(.caption)
                }
            }
        }
        .navigationTitle("Library")
        .refreshable { await loadStats() }
        .task { await loadStats() }
    }

    private func loadStats() async {
        guard !isLoading else { return }
        isLoading = true
        defer { isLoading = false }
        do {
            let response: LibraryStatsResponse = try await config.client.execute(query: GQL.libraryStats)
            stats = response.libraryStats
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }
}

private struct StatRow: View {
    let label: String
    let value: String

    var body: some View {
        HStack {
            Text(label)
            Spacer()
            Text(value)
                .foregroundStyle(.secondary)
        }
    }
}
