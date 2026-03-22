import SwiftUI

struct AlbumDetailView: View {
    @Environment(ServerConfig.self) private var config
    let album: Album

    @State private var tracks: [Track] = []
    @State private var isLoading = false
    @State private var hasNextPage = false
    @State private var endCursor: String?
    @State private var error: String?

    private let pageSize = 100

    var body: some View {
        List {
            Section {
                albumHeader
            }
            .listRowInsets(EdgeInsets())
            .listRowBackground(Color.clear)

            Section {
                ForEach(tracks) { track in
                    TrackRowView(track: track, showAlbum: false) {
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
            }

            if let error {
                Section {
                    Text(error)
                        .foregroundStyle(.red)
                        .font(.caption)
                }
            }
        }
        .listStyle(.plain)
        .navigationTitle(album.title)
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        #endif
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Menu {
                    Button {
                        Task { await config.client.replaceQueue(trackIds: tracks.map(\.id)) }
                    } label: {
                        Label("Play Album", systemImage: "play.fill")
                    }
                    Button {
                        Task { await config.client.addToQueue(trackIds: tracks.map(\.id)) }
                    } label: {
                        Label("Add to Queue", systemImage: "text.append")
                    }
                } label: {
                    Image(systemName: "ellipsis.circle")
                }
            }
        }
        .refreshable { await reload() }
        .task { await reload() }
    }

    private var albumHeader: some View {
        VStack(spacing: 8) {
            AsyncImage(url: URL(string: "\(config.baseURL)/cover/album/\(album.id)")) { phase in
                switch phase {
                case .success(let image):
                    image
                        .resizable()
                        .aspectRatio(1, contentMode: .fit)
                case .failure:
                    albumPlaceholder
                default:
                    albumPlaceholder
                }
            }
            .frame(width: 200, height: 200)
            .clipShape(RoundedRectangle(cornerRadius: 8))
            .shadow(radius: 4)

            Text(album.title)
                .font(.title2)
                .fontWeight(.bold)
                .multilineTextAlignment(.center)

            Text(album.artistName)
                .font(.subheadline)
                .foregroundStyle(.secondary)

            HStack(spacing: 8) {
                if let year = album.year {
                    Text(year)
                }
                if let codec = album.codec {
                    Text(codec)
                }
                Text("\(album.trackCount) tracks")
                Text(album.formattedDuration)
            }
            .font(.caption)
            .foregroundStyle(.secondary)

            Button {
                Task { await config.client.replaceQueue(trackIds: tracks.map(\.id)) }
            } label: {
                Label("Play", systemImage: "play.fill")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.large)
            .padding(.horizontal, 40)
            .padding(.top, 8)
        }
        .padding()
        .frame(maxWidth: .infinity)
    }

    private var albumPlaceholder: some View {
        Rectangle()
            .fill(.quaternary)
            .aspectRatio(1, contentMode: .fit)
            .overlay {
                Image(systemName: "music.note")
                    .font(.largeTitle)
                    .foregroundStyle(.tertiary)
            }
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

        var variables: [String: Any] = [
            "albumId": album.id,
            "first": pageSize,
            "favouritesOnly": false,
        ]
        if let endCursor { variables["after"] = endCursor }

        do {
            let response: TracksResponse = try await config.client.execute(
                query: GQL.tracks, variables: variables
            )
            tracks.append(contentsOf: response.tracks.nodes)
            hasNextPage = response.tracks.pageInfo.hasNextPage
            endCursor = response.tracks.endCursor
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }

}
