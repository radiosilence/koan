import AppKit
import KoanFFI
import SwiftUI
import UniformTypeIdentifiers

struct SidebarView: View {
    @Environment(LibraryModel.self) private var library
    @Environment(Navigator.self) private var nav
    @Environment(PlayerModel.self) private var player
    @Environment(SearchModel.self) private var search
    @Environment(UIState.self) private var ui
    @Environment(PlaylistsModel.self) private var playlists
    @FocusState private var searchFocused: Bool
    /// Highlights the Queue row while something is held over it — without it a
    /// drop is a guess.
    @State private var queueDropTargeted = false
    /// Lit while something is held over the Playlists heading, which is where a
    /// drop makes a new playlist.
    @State private var newPlaylistDropTargeted = false
    /// The playlist a drop is hovering over, so only that row lights up.
    @State private var playlistDropTarget: Int64?
    /// Tracks waiting for a name. Set by a drop on the heading; the sheet that
    /// takes the name is what finally creates the playlist.
    @State private var naming: [PlayableTransfer]?
    @State private var newName = ""
    /// The playlist being renamed, and what it is being renamed to.
    @State private var renaming: Playlist?
    @State private var renameTo = ""

    var body: some View {
        @Bindable var search = search

        // The lit row is the section you are in, whatever you have pushed on
        // top of it — that is where Back returns you to. The navigator owns
        // both halves of the binding; see `sidebarSelection` for why the
        // highlight is not derived from the stack.
        List(selection: nav.sidebarSelection) {
            Section {
                HStack {
                    Label("Queue", systemImage: "list.bullet")
                    if player.isBusy {
                        Spacer()
                        ProgressView().controlSize(.small)
                    }
                }
                    .tag(Navigator.Section.queue)
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
                        .tag(Navigator.Section.searchResults)
                }
            }

            Section("Library") {
                Label("Albums", systemImage: "square.stack")
                    .tag(Navigator.Section.albums)
                Label("Artists", systemImage: "music.mic")
                    .tag(Navigator.Section.artists)
                Label("Favourites", systemImage: "heart")
                    .tag(Navigator.Section.favourites)
                Label("History", systemImage: "clock.arrow.circlepath")
                    .tag(Navigator.Section.playHistory)
            }

            playlistSection
        }
        .listStyle(.sidebar)
        // The List's own hooks rather than per-row gestures, the same way the
        // queue and every track list does it: wired into selection, so the
        // double-click does not steal the click that selects the row. Only
        // playlists answer to either — the other rows are places, and a place
        // has nothing to play or rename.
        .contextMenu(forSelectionType: Navigator.Section.self) { sections in
            if sections.count == 1,
               case .playlist(let id) = sections.first,
               let playlist = playlists.playlist(id: id) {
                menu(for: playlist)
            }
        } primaryAction: { sections in
            if case .playlist(let id) = sections.first,
               let playlist = playlists.playlist(id: id) {
                play(playlist)
            }
        }
        // The footer floats over the rows rather than being fenced off by a
        // divider; the soft edge fades a row out as it passes underneath.
        .scrollEdgeEffectStyle(.soft, for: .bottom)
        // The field belongs to the sidebar, not the window: in the toolbar it
        // sat on top of the lyrics inspector.
        .searchable(text: $search.query, placement: .sidebar, prompt: "Search")
        .searchSuggestions { SearchSuggestions() }
        .searchFocused($searchFocused)
        // `/`, the way it works in the TUI. The field is somewhere else on
        // screen, so the key can only ask for it by token.
        .onChange(of: ui.searchFocusToken) { _, _ in
            searchFocused = true
        }
        .safeAreaInset(edge: .bottom) { footer }
        // A drop onto the heading has tracks but no name yet; this is where it
        // gets one, and only then does the playlist exist.
        .alert("New Playlist", isPresented: Binding(
            get: { naming != nil },
            set: { if !$0 { naming = nil } }
        )) {
            TextField("Name", text: $newName)
            Button("Cancel", role: .cancel) {
                naming = nil
                newName = ""
            }
            Button("Create") {
                let dropped = naming ?? []
                let name = newName
                naming = nil
                newName = ""
                Task {
                    guard let created = await playlists.create(named: name, dropped: dropped)
                    else { return }
                    nav.open(playlist: created.id)
                }
            }
        } message: {
            Text(namingMessage)
        }
        .alert("Rename Playlist", isPresented: Binding(
            get: { renaming != nil },
            set: { if !$0 { renaming = nil } }
        )) {
            TextField("Name", text: $renameTo)
            Button("Cancel", role: .cancel) { renaming = nil }
            Button("Rename") {
                if let renaming { playlists.rename(id: renaming.id, to: renameTo) }
                renaming = nil
            }
        }
        .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { ui.sidebarWidth = $0 }
        .onDisappear { ui.sidebarWidth = 0 }
    }

    // MARK: - Playlists

    /// The playlists, in the order they were arranged.
    ///
    /// The heading is itself a drop target: dropping onto it asks for a name and
    /// makes a new playlist, which is the shortest path from "these tracks" to
    /// "a playlist of these tracks".
    @ViewBuilder
    private var playlistSection: some View {
        Section {
            ForEach(playlists.playlists, id: \.id) { playlist in
                PlaylistRow(
                    playlist: playlist,
                    covers: playlists.covers[playlist.id] ?? []
                )
                    .tag(Navigator.Section.playlist(playlist.id))
                    .dropDestination(for: PlayableTransfer.self) { dropped, _ in
                        playlists.add(dropped: dropped, to: playlist.id)
                        return true
                    } isTargeted: { targeted in
                        playlistDropTarget = targeted ? playlist.id : nil
                    }
                    .listRowBackground(
                        playlistDropTarget == playlist.id
                            ? RoundedRectangle(cornerRadius: 5).fill(.tint.opacity(0.25))
                            : nil
                    )
            }
            .onMove { source, destination in
                var order = playlists.playlists.map(\.id)
                order.move(fromOffsets: source, toOffset: destination)
                playlists.reorder(to: order)
            }
        } header: {
            Text("Playlists")
                // Full width so the whole heading takes a drop, not just the
                // seven characters of the word.
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
                .dropDestination(for: PlayableTransfer.self) { dropped, _ in
                    naming = dropped
                    return true
                } isTargeted: { newPlaylistDropTargeted = $0 }
                .background(
                    newPlaylistDropTargeted
                        ? RoundedRectangle(cornerRadius: 4).fill(.tint.opacity(0.25))
                        : nil
                )
        }
    }

    @ViewBuilder
    private func menu(for playlist: Playlist) -> some View {
        Button("Play") { play(playlist) }
        Button("Shuffle") { play(playlist, shuffled: true) }
        Divider()
        Button("Rename…") {
            renameTo = playlist.name
            renaming = playlist
        }
        Button("Export as M3U8…") { export(playlist) }
        Divider()
        Button("Delete", role: .destructive) {
            playlists.delete(id: playlist.id)
            // A deleted playlist is not somewhere Back can return to.
            nav.forget(.playlist(playlist.id))
            if nav.section == .playlist(playlist.id) { nav.show(.queue) }
        }
    }

    /// A save panel rather than SwiftUI's `fileExporter`: the exporter wants a
    /// document to write, and the file is written by the engine — only it knows
    /// which tracks have a file on this machine to point at.
    private func export(_ playlist: Playlist) {
        let panel = NSSavePanel()
        panel.allowedContentTypes = [UTType(filenameExtension: "m3u8") ?? .plainText]
        panel.nameFieldStringValue = "\(playlist.name).m3u8"
        panel.canCreateDirectories = true
        guard panel.runModal() == .OK, let url = panel.url else { return }
        playlists.export(id: playlist.id, to: url)
    }

    private func play(_ playlist: Playlist, shuffled: Bool = false) {
        let engine = playlists.engine
        Task {
            _ = try? await engine.playPlaylist(
                playlistId: playlist.id, startAt: nil, shuffled: shuffled
            )
            nav.show(.queue)
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

    private var namingMessage: String {
        let count = naming?.count ?? 0
        return count == 1
            ? "One thing dropped in. Give the playlist a name."
            : "\(count) things dropped in. Give the playlist a name."
    }

    /// Library size and scan state. The counts are the quickest way to tell
    /// whether a scan actually picked anything up.
    @ViewBuilder
    private var footer: some View {
        VStack(alignment: .leading, spacing: 6) {
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



/// One playlist in the sidebar: its mosaic, its name, and how much is in it.
private struct PlaylistRow: View {
    let playlist: Playlist
    let covers: [AlbumArtwork.Source]

    var body: some View {
        HStack(spacing: 8) {
            PlaylistArtwork(sources: covers, cornerRadius: 3)
                .frame(width: 24, height: 24)

            VStack(alignment: .leading, spacing: 0) {
                Text(playlist.name)
                    .lineLimit(1)
                Text(Format.count(Int64(playlist.trackCount), "track"))
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
        }
        // Full width, so the drop target is the row rather than the text — the
        // same reason the Queue row does it.
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
    }
}
