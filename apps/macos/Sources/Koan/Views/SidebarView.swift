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
    @Environment(EngineMirror.self) private var mirror
    @FocusState private var searchFocused: Bool
    /// Highlights the Queue row while something is held over it — without it a
    /// drop is a guess.
    @State private var queueDropTargeted = false
    /// Lit while something is held over "New Playlist…", which is where a drop
    /// makes one.
    @State private var newPlaylistDropTargeted = false
    /// The playlist a drop is hovering over, so only that row lights up.
    @State private var playlistDropTarget: Int64?
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
                    Label("Queue", systemImage: Icon.queueSection)
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
                    Label("Results", systemImage: Icon.search)
                        .tag(Navigator.Section.searchResults)
                }
            }

            Section("Library") {
                Label("Albums", systemImage: Icon.album)
                    .tag(Navigator.Section.albums)
                Label("Artists", systemImage: Icon.artist)
                    .tag(Navigator.Section.artists)
                Label("Favourites", systemImage: Icon.favourite)
                    .tag(Navigator.Section.favourites)
                Label("History", systemImage: Icon.history)
                    .tag(Navigator.Section.playHistory)
                HStack {
                    Label("Downloads", systemImage: Icon.downloads)
                    // Only while something is happening. A zero sitting there
                    // permanently is a number nobody reads.
                    if mirror.activeTransfers > 0 {
                        Spacer()
                        Text("\(mirror.activeTransfers)")
                            .font(.caption.monospacedDigit())
                            .foregroundStyle(.secondary)
                    }
                }
                .tag(Navigator.Section.downloads)
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

    /// The playlists, in the order they were arranged, and a standing row for
    /// making another.
    ///
    /// Every row here is built like the Queue row above, because that is the
    /// one drop target in this window that has ever worked. What broke the
    /// others was structure, not payload: `ForEach.onMove` takes over dropping
    /// for the rows it covers, a `Section` header is not a row a `List` will
    /// deliver to, and a `Button` swallows the drag before it lands. So there
    /// is no `onMove` — reordering rides the same drop as everything else, on
    /// a payload that says which playlist it is — and no button.
    @ViewBuilder
    private var playlistSection: some View {
        Section("Playlists") {
            ForEach(playlists.playlists, id: \.id) { playlist in
                PlaylistRow(
                    playlist: playlist,
                    covers: playlists.covers[playlist.id] ?? []
                )
                    .tag(Navigator.Section.playlist(playlist.id))
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .contentShape(Rectangle())
                    // Dragging a playlist somewhere else means its tracks —
                    // onto the queue, onto another playlist. Dropping it back
                    // into this list means where it sits.
                    .draggable(PlayableTransfer(
                        kind: .playlist, id: playlist.id, name: playlist.name
                    ))
                    .dropDestination(for: PlayableTransfer.self) { dropped, _ in
                        accept(dropped, on: playlist)
                        return true
                    } isTargeted: { targeted in
                        playlistDropTarget = targeted ? playlist.id : nil
                    }
                    .dropDestination(for: URL.self) { urls, _ in
                        playlists.add(files: urls, to: playlist.id)
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

            newPlaylistRow
        }
    }

    /// A drop landed on a playlist: either another playlist being put in its
    /// place, or things to add to it.
    private func accept(_ dropped: [PlayableTransfer], on playlist: Playlist) {
        let moving = dropped.filter { $0.kind == .playlist }.map(\.id)
        if !moving.isEmpty {
            playlists.reorder(moving: moving, onto: playlist.id)
        }
        let adding = dropped.filter { $0.kind != .playlist }
        if !adding.isEmpty {
            playlists.add(dropped: adding, to: playlist.id)
        }
    }

    /// Make one — by clicking, or by dropping something on it.
    ///
    /// A row rather than a button, for the reason above: a button never gets
    /// the drop. Which means the click is a tap gesture, and that is safe here
    /// only because the row takes no selection — on a selectable row it would
    /// be racing the gesture that selects it.
    private var newPlaylistRow: some View {
        Label("New Playlist…", systemImage: "plus")
            .foregroundStyle(.secondary)
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
            .selectionDisabled()
            .onTapGesture { playlists.naming = [] }
            .dropDestination(for: PlayableTransfer.self) { dropped, _ in
                playlists.beginNaming(dropped: dropped)
                return true
            } isTargeted: { newPlaylistDropTargeted = $0 }
            .dropDestination(for: URL.self) { urls, _ in
                playlists.beginNaming(files: urls)
                return true
            } isTargeted: { newPlaylistDropTargeted = $0 }
            .listRowBackground(
                newPlaylistDropTargeted
                    ? RoundedRectangle(cornerRadius: 5).fill(.tint.opacity(0.25))
                    : nil
            )
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

    /// Play it where you stand. Double-clicking a row selects it first, so you
    /// land on the playlist itself and watch it start — and playing something
    /// is not on its own a reason to be moved anywhere.
    private func play(_ playlist: Playlist, shuffled: Bool = false) {
        let engine = playlists.engine
        Task {
            _ = try? await engine.playPlaylist(
                playlistId: playlist.id, startAt: nil, shuffled: shuffled
            )
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
                Label(radioStatus, systemImage: Icon.radio)
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
