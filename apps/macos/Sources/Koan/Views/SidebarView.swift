import AppKit
import KoanFFI
import SwiftUI

struct SidebarView: View {
    @Environment(LibraryModel.self) private var library
    @Environment(PlayerModel.self) private var player
    @Environment(SearchModel.self) private var search
    @Environment(UIState.self) private var ui
    @FocusState private var searchFocused: Bool
    /// Highlights the Queue row while something is held over it — without it a
    /// drop is a guess.
    @State private var queueDropTargeted = false

    var body: some View {
        @Bindable var library = library
        @Bindable var search = search

        List(selection: Binding(
            get: { library.navSelection },
            set: { chosen in
                guard let chosen else { return }
                // Choosing a section always lands on its root — including the
                // one you are already in, which is otherwise a trip out to
                // another section and back. `navSelection` follows the detail
                // stack, so this also covers choosing Albums from inside an
                // album you reached through Favourites.
                library.section = chosen
                library.popToRoot()
            }
        )) {
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
                Label("History", systemImage: "clock.arrow.circlepath")
                    .tag(LibraryModel.Section.playHistory)
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
        .searchFocused($searchFocused)
        // `/`, the way it works in the TUI. The field is somewhere else on
        // screen, so the key can only ask for it by token.
        .onChange(of: ui.searchFocusToken) { _, _ in
            NSLog("SIDEBAR token change -> searchFocused = true")
            searchFocused = true
        }
        .onChange(of: searchFocused) { _, new in NSLog("SIDEBAR searchFocused = \(new)") }
        .safeAreaInset(edge: .bottom) {
            footer
        }
    }

    /// What radio is about to do, rather than that it is switched on.
    private var radioStatus: String {
        guard let cursor = player.currentItemId,
              let index = player.queue.firstIndex(where: { $0.queueItemId == cursor })
        else {
            return "Radio — waiting for something to play"
        }
        let ahead = player.queue.count - index - 1
        return ahead == 0
            ? "Radio — extending after this track"
            : "Radio — \(Format.count(Int64(ahead), "track")) ahead"
    }

    /// Library size and scan state. The counts are the quickest way to tell
    /// whether a scan actually picked anything up.
    @ViewBuilder
    private var footer: some View {
        VStack(alignment: .leading, spacing: 6) {
            Divider()

            // Every long task, one row each. Replaces a "Scanning…" line that
            // said the same thing whatever was actually running.
            ActivityList()

            if let stats = library.stats {
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
                // "Radio on" only repeats what the lit button already says.
                // What is worth knowing is whether it is about to do anything,
                // which is a question about how much queue is left.
                Label(radioStatus, systemImage: "dot.radiowaves.left.and.right")
                    .font(.caption)
                    .foregroundStyle(.tint)
            }
        }
        .padding(.horizontal, 14)
        .padding(.bottom, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

