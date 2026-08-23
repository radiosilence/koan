import KoanFFI
import SwiftUI

struct SidebarView: View {
    @Environment(LibraryModel.self) private var library
    @Environment(PlayerModel.self) private var player
    @Environment(SearchModel.self) private var search
    /// Highlights the Queue row while something is held over it — without it a
    /// drop is a guess.
    @State private var queueDropTargeted = false

    var body: some View {
        @Bindable var library = library
        @Bindable var search = search

        List(selection: $library.section) {
            Section {
                HStack {
                    Label("Queue", systemImage: "list.bullet")
                    if player.isBusy {
                        Spacer()
                        ProgressView().controlSize(.small)
                    }
                }
                    .tag(LibraryModel.Section.queue)
                    // Full width, so the target is the row rather than just the
                    // text — dropping onto the empty part of the row should
                    // work, and a target you have to hit precisely is no target.
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .contentShape(Rectangle())
                    .dropDestination(for: PlayableTransfer.self) { dropped, _ in
                        player.acceptDrop(dropped)
                        return true
                    } isTargeted: { targeted in
                        queueDropTargeted = targeted
                    }
                    .listRowBackground(
                        queueDropTargeted
                            ? RoundedRectangle(cornerRadius: 5).fill(.tint.opacity(0.25))
                            : nil
                    )
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
        // The field belongs to the sidebar, not the window: in the toolbar it
        // sat on top of the lyrics inspector.
        .searchable(text: $search.query, placement: .sidebar, prompt: "Search")
        .searchSuggestions { SearchSuggestions() }
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

            if player.radioEnabled {
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
