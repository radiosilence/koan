#if canImport(AppKit)
import AppKit
#else
import UIKit
#endif
import KoanFFI
import SwiftUI
import UniformTypeIdentifiers

struct SidebarView: View {
    @Environment(Navigator.self) private var nav
    @Environment(PlayerModel.self) private var player
    @Environment(SearchModel.self) private var search
    @Environment(UIState.self) private var ui
    @Environment(PlaylistsModel.self) private var playlists
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

    /// How far down the rows start under the theme's search field.
    @State private var searchHeight: CGFloat = 0

    var body: some View {
        chrome
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
    }

    @ViewBuilder
    private var chrome: some View {
        #if os(macOS)
        if KoanTheme.isOn { themed } else { system }
        #else
        system
        #endif
    }

    #if os(macOS)
    /// The theme's sidebar: its own search field over the rows, and the footer
    /// below a hairline. Stacked rather than laid over the list as bars, so
    /// rows stop at each edge instead of passing beneath on a ground of the
    /// platform's.
    private var themed: some View {
        ZStack(alignment: .top) {
            VStack(spacing: 0) {
                Color.clear.frame(height: searchHeight)
                list.scrollEdgeEffectHidden(true, for: .all)
                Rectangle().fill(Color.koanRowRule).frame(height: KoanTheme.hairline)
                SidebarFooter()
            }
            SidebarSearch(fieldHeight: $searchHeight)
        }
        // Under the field and the footer as well as the rows.
        .koanSidebar()
    }
    #endif

    /// The platform's sidebar: the system's search field, and the footer as a
    /// bar over the rows.
    private var system: some View {
        @Bindable var search = search
        return list
            // The footer is text over text, so the rows passing beneath it get
            // the hard edge: the sidebar behind the footer and a line between
            // them. It applies only under a bar (`safeAreaBar` below); a plain
            // `safeAreaInset` takes no edge effect, and the rows showed through.
            .scrollEdgeEffectStyle(.hard, for: .bottom)
            .scrollEdgeEffectHidden(false, for: .top)
            // The field belongs to the sidebar, not the window: in the toolbar
            // it would sit on top of the lyrics inspector.
            .searchable(text: $search.query, placement: .sidebar, prompt: "Search") // theme: raw — the platform's look; the theme's is `SidebarSearch`
            .searchSuggestions { SearchSuggestions() }
            .searchFocused($searchFocused)
            // `/`, the way it works in the TUI. The field is somewhere else on
            // screen, so the key can only ask for it by token.
            .onChange(of: ui.searchFocusToken) { _, _ in
                searchFocused = true
            }
            .safeAreaBar(edge: .bottom) { SidebarFooter() }
    }

    private var list: some View {
        // A row is lit when the page on screen is that row. The navigator
        // owns both halves of the binding — see `sidebarSelection`.
        List(selection: nav.sidebarSelection) {
            Section {
                QueueRowLabel()
                    .tag(Navigator.Section.queue)
                    .koanNavRow(selected: nav.section == .queue)
                    // Full width, so the target is the row rather than just the
                    // text — dropping onto the empty part of the row should
                    // work, and a target you have to hit precisely is no target.
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .contentShape(Rectangle())
                    .dropTarget(for: PlayableTransfer.self) { dropped, _ in
                        player.acceptDrop(dropped)
                        return true
                    } isTargeted: { targeted in
                        queueDropTargeted = targeted
                    }
                    .dropHighlight(queueDropTargeted)
                if search.hasQuery {
                    KoanLabel("Results", icon: Icon.search)
                        .tag(Navigator.Section.searchResults)
                }
            }

            Section {
                KoanLabel("Albums", icon: Icon.album)
                    .sidebarRow(.albums)
                KoanLabel("Artists", icon: Icon.artist)
                    .sidebarRow(.artists)
                KoanLabel("Tracks", icon: Icon.track)
                    .sidebarRow(.tracks)
                KoanLabel("Favourites", icon: Icon.favourite)
                    .sidebarRow(.favourites)
                KoanLabel("Recently Played", icon: Icon.recentlyPlayed)
                    .sidebarRow(.recentlyPlayed)
                KoanLabel("Downloaded", icon: Icon.onDevice)
                    .sidebarRow(.onDevice)
                KoanLabel("History", icon: Icon.history)
                    .sidebarRow(.playHistory)
                DownloadsRowLabel()
                    .sidebarRow(.downloads)
            } header: {
                KoanSectionHeader("Library")
            }

            playlistSection
        }
        .listStyle(.sidebar)
        // The theme's sidebar is flat ground, not the system's material.
        .koanSidebar()
        // The List's own hooks rather than per-row gestures, the same way the
        // queue and every track list does it: wired into selection, so the
        // double-click does not steal the click that selects the row. Only
        // playlists answer to either — the other rows are places, and a place
        // has nothing to play or rename.
        .selectionMenu(for: Navigator.Section.self) { sections in
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
    }


    // MARK: - Playlists

    /// The playlists, in the order they were arranged, and a standing row for
    /// making another.
    ///
    /// Every row here is built like the Queue row above. What decides whether
    /// a `List` row takes a drop is structure: `ForEach.onMove` takes over
    /// dropping for the rows it covers, a `Section` header is not a row a `List` will
    /// deliver to, and a `Button` swallows the drag before it lands. So there
    /// is no `onMove` — reordering rides the same drop as everything else, on
    /// a payload that says which playlist it is — and no button.
    @ViewBuilder
    private var playlistSection: some View {
        Section {
            ForEach(playlists.playlists, id: \.id) { playlist in
                PlaylistRow(
                    playlist: playlist,
                    covers: playlists.covers[playlist.id] ?? []
                )
                    .tag(Navigator.Section.playlist(playlist.id))
                    .koanNavRow(selected: nav.section == .playlist(playlist.id))
                    // Dragging a playlist somewhere else means its tracks —
                    // onto the queue, onto another playlist. Dropping it back
                    // into this list means where it sits.
                    .dragSource(PlayableTransfer(
                        kind: .playlist, id: playlist.id, name: playlist.name
                    ))
                    .dropTarget(for: PlayableTransfer.self) { dropped, _ in
                        accept(dropped, on: playlist)
                        return true
                    } isTargeted: { targeted in
                        // Guarded: the row being left can report after the one entered.
                        playlistDropTarget = targeted
                            ? playlist.id
                            : (playlistDropTarget == playlist.id ? nil : playlistDropTarget)
                    }
                    .dropHighlight(playlistDropTarget == playlist.id)
            }

            newPlaylistRow
        } header: {
            KoanSectionHeader("Playlists")
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
        KoanLabel("New Playlist…", icon: "plus")
            .foregroundStyle(KoanTheme.style(.muted))
            .koanNavRow(selected: false)
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
            .selectionDisabled()
            .onTapGesture { playlists.naming = [] }
            .accessibilityAddTraits(.isButton)
            .accessibilityAction { playlists.naming = [] }
            .dropTarget(for: PlayableTransfer.self) { dropped, _ in
                playlists.beginNaming(dropped: dropped)
                return true
            } isTargeted: { newPlaylistDropTargeted = $0 }
            .dropHighlight(newPlaylistDropTargeted)
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
        .disabled(playlist.fromFile)
        #if os(macOS)
        Button("Export as M3U8…") { export(playlist) }
        #endif
        Divider()
        Button("Delete", role: .destructive) {
            playlists.delete(id: playlist.id)
            // A deleted playlist is not somewhere Back can return to.
            nav.forget(.playlist(playlist.id))
            if nav.section == .playlist(playlist.id) { nav.show(.queue) }
        }
    }

    #if os(macOS)
    /// A save panel rather than SwiftUI's `fileExporter`: the exporter wants a
    /// document to write, and the file is written by the engine — only it knows
    /// which tracks have a file on this machine to point at.
    ///
    /// There is no save panel on iOS and no obvious place to put the file, so
    /// the affordance is absent there rather than half-present.
    private func export(_ playlist: Playlist) {
        let panel = NSSavePanel()
        panel.allowedContentTypes = [UTType(filenameExtension: "m3u8") ?? .plainText]
        panel.nameFieldStringValue = "\(playlist.name).m3u8"
        panel.canCreateDirectories = true
        guard panel.runModal() == .OK, let url = panel.url else { return }
        playlists.export(id: playlist.id, to: url)
    }
    #endif

    /// Play it where you stand. Double-clicking a row selects it first, so you
    /// land on the playlist itself and watch it start — and playing something
    /// is not on its own a reason to be moved anywhere.
    private func play(_ playlist: Playlist, shuffled: Bool = false) {
        let engine = playlists.engine
        Task {
            _ = try? await engine.playPlaylist(
                playlistId: playlist.id, startEntry: nil, shuffled: shuffled
            )
        }
    }
}

/// Library size and what koan is doing. Its own view because it reads the
/// running tasks, and read in `SidebarView` those would re-run the sidebar as
/// each one moves.
private struct SidebarFooter: View {
    @Environment(LibraryModel.self) private var library

    /// Library size and scan state. The counts are the quickest way to tell
    /// whether a scan picked anything up.
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            // Every long task, one row each.
            ActivityList()

            if let stats = library.stats {
                VStack(alignment: .leading, spacing: 2) {
                    Text(Format.count(stats.totalTracks, "track"))
                    Text("\(Format.count(stats.totalAlbums, "album")) · \(Format.count(stats.totalArtists, "artist"))")
                    if stats.remoteTracks > 0 {
                        Text("\(stats.cachedTracks.formatted(.number)) of \(stats.remoteTracks.formatted(.number)) remote cached")
                    }
                }
                .koanText(.meta, .muted)
            }
        }
        // In the theme: the rows' own inset at the sides, and clear of the
        // window's rounded corner below. The platform's look as it was.
        .padding(.horizontal, KoanTheme.metric(KoanTheme.Space.l, system: 14))
        .padding(.top, KoanTheme.metric(KoanTheme.Space.s, system: 0))
        .padding(.bottom, KoanTheme.metric(KoanTheme.Space.xl, system: 10))
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

// The two rows that show something live, each reading it for itself — read in
// `SidebarView`, a transfer finishing would re-run the whole sidebar.

private struct QueueRowLabel: View {
    @Environment(PlayerModel.self) private var player

    var body: some View {
        HStack {
            KoanLabel("Queue", icon: Icon.queueSection)
            if player.isBusy {
                Spacer()
                ProgressView().controlSize(.small)
            }
        }
    }
}

private struct DownloadsRowLabel: View {
    @Environment(EngineMirror.self) private var mirror

    var body: some View {
        HStack {
            KoanLabel("Downloads", icon: Icon.downloads)
            // Only while something is happening. A zero sitting there
            // permanently is a number nobody reads.
            if mirror.activeTransfers > 0 {
                Spacer()
                Text("\(mirror.activeTransfers)")
                    .koanText(.fine, .muted)
                    .monospacedDigit()
            }
        }
    }
}

private extension View {
    /// Lights a row while a drop is held over it.
    func dropHighlight(_ lit: Bool) -> some View {
        listRowBackground(lit ? RoundedRectangle(cornerRadius: KoanTheme.radius(5)).fill(.tint.opacity(0.25)) : nil)
    }

    /// A row that is a place: selecting it goes there, and clicking it while
    /// already there goes back to the top.
    ///
    /// Selection alone cannot see the second click — the `List` reports a
    /// change, and clicking the selected row changes nothing — so the tap rides
    /// alongside it. Simultaneous, so it never takes the click that selects.
    func sidebarRow(_ section: Navigator.Section) -> some View {
        modifier(SidebarRow(section: section))
    }
}

private struct SidebarRow: ViewModifier {
    let section: Navigator.Section
    @Environment(Navigator.self) private var nav

    func body(content: Content) -> some View {
        content
            .tag(section)
            .koanNavRow(selected: nav.section == section)
            .simultaneousGesture(TapGesture().onEnded { nav.rewind(section) })
    }
}
