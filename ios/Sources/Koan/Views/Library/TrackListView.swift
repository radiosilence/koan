import SwiftUI

struct TrackListView: View {
    let albumId: Int
    let albumTitle: String

    @State private var tracks: [Track] = []
    @State private var isLoading = false
    @State private var error: String?

    private var client: GraphQLClient { .shared }

    var body: some View {
        Group {
            if isLoading && tracks.isEmpty {
                ProgressView("Loading tracks...")
            } else if let error {
                ContentUnavailableView {
                    Label("Error", systemImage: "exclamationmark.triangle")
                } description: {
                    Text(error)
                } actions: {
                    Button("Retry") { Task { await loadTracks() } }
                }
            } else if tracks.isEmpty {
                ContentUnavailableView("No Tracks", systemImage: "music.note",
                                       description: Text("No tracks found on this album."))
            } else {
                List(tracks) { track in
                    Button {
                        Task { await playTrack(track) }
                    } label: {
                        HStack {
                            if let num = track.trackNumber {
                                Text("\(num)")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                                    .frame(width: 28, alignment: .trailing)
                            }
                            VStack(alignment: .leading) {
                                Text(track.title)
                                    .font(.body)
                                if track.artist != tracks.first?.artist {
                                    Text(track.artist)
                                        .font(.caption)
                                        .foregroundStyle(.secondary)
                                }
                            }
                            Spacer()
                            Text(track.durationFormatted)
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                    .tint(.primary)
                }
            }
        }
        .navigationTitle(albumTitle)
        .refreshable { await loadTracks() }
        .task { await loadTracks() }
    }

    private func loadTracks() async {
        isLoading = true
        defer { isLoading = false }

        do {
            let response: TracksResponse = try await client.execute(
                query: GQL.tracksForAlbum,
                variables: ["albumId": albumId])
            tracks = response.tracks.nodes
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }

    private func playTrack(_ track: Track) async {
        let trackIds = tracks.map(\.id)
        let _: ReplaceQueueResponse? = try? await client.execute(
            query: GQL.replaceQueue,
            variables: ["trackIds": trackIds])
    }
}
