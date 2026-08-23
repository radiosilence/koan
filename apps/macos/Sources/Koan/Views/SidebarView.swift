import KoanFFI
import SwiftUI

struct SidebarView: View {
    @Environment(LibraryModel.self) private var library
    @Environment(PlayerModel.self) private var player
    @Environment(SearchModel.self) private var search

    var body: some View {
        @Bindable var library = library

        List(selection: $library.section) {
            Section {
                Label("Queue", systemImage: "list.bullet")
                    .tag(LibraryModel.Section.queue)
                if search.hasQuery {
                    Label("Results", systemImage: "magnifyingglass")
                        .tag(LibraryModel.Section.searchResults)
                }
            }

            Section("Library") {
                Label("Albums", systemImage: "square.stack")
                    .tag(LibraryModel.Section.albums)
                Label("Artists", systemImage: "music.mic")
                    .tag(LibraryModel.Section.artists)
                Label("Favourites", systemImage: "heart")
                    .tag(LibraryModel.Section.favourites)
            }

            Section("Playlists") {
                Label("Snapshots", systemImage: "bookmark")
                    .tag(LibraryModel.Section.snapshots)
            }
        }
        .listStyle(.sidebar)
        .safeAreaInset(edge: .bottom) {
            footer
        }
    }

    /// Library size and scan state. The counts are the quickest way to tell
    /// whether a scan actually picked anything up.
    @ViewBuilder
    private var footer: some View {
        VStack(alignment: .leading, spacing: 6) {
            Divider()

            if library.isScanning {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Scanning…")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            } else if let stats = library.stats {
                VStack(alignment: .leading, spacing: 2) {
                    Text(Format.count(stats.totalTracks, "track"))
                    Text("\(Format.count(stats.totalAlbums, "album")) · \(Format.count(stats.totalArtists, "artist"))")
                    if stats.remoteTracks > 0 {
                        Text("\(stats.cachedTracks.formatted(.number)) of \(stats.remoteTracks.formatted(.number)) remote cached")
                    }
                }
                .font(.caption)
                .foregroundStyle(.secondary)
            }

            if player.nowPlaying.radioEnabled {
                Label("Radio on", systemImage: "dot.radiowaves.left.and.right")
                    .font(.caption)
                    .foregroundStyle(.tint)
            }
        }
        .padding(.horizontal, 14)
        .padding(.bottom, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}
