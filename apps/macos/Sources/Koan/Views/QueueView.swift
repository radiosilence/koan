import KoanFFI
import SwiftUI

/// The queue is the main stage, the way it is in the TUI — the library is
/// somewhere you visit to feed this, not the other way round.
///
/// Tracks are grouped under album headers, and the grouping follows *contiguous
/// runs* rather than sorting: queue order is the user's, and collapsing two
/// separate visits to the same record into one heading would misrepresent it.
struct QueueView: View {
    @Environment(PlayerModel.self) private var player
    @Environment(Navigator.self) private var nav
    @Environment(LibraryModel.self) private var library
    @Environment(OrganizeModel.self) private var organize
    @Environment(\.openWindow) private var openWindow
    @Environment(UIState.self) private var ui
    @Environment(PlaylistsModel.self) private var playlists
    /// The queue outlives the page you are on — see `StageView`. Anything
    /// aimed at whatever list is in front of you has to check.
    @Environment(\.onStage) private var onStage


    /// Grouped or one row per track. Persisted because it is a preference about
    /// how you listen rather than about the queue in front of you: an album
    /// listen wants the headings, a long shuffled queue wants every row to say
    /// what it is and show its own sleeve.
    @AppStorage("queueGrouped") private var grouped = true

    /// Selection is local `@State`, deliberately.
    ///
    /// It lived on `PlayerModel` so the Edit menu could reach it, but reading an
    /// observable in the body means every selection change invalidates the whole
    /// view and rebuilds the List — under the very click that caused it, which
    /// is what made clicking here so unreliable. It is mirrored to the model on
    /// change instead: written, never read, so it stays out of the render path.
    @State private var selection: Set<String> = []

    /// Album headings are rows in their own right, not decoration attached to
    /// the first track. That is what lets an album be selected and dragged as a
    /// unit — and stops selecting a track from lighting up the heading above it,
    /// which is what happened while they shared a row.
    private var rows: [Row] {
        grouped ? Row.build(from: player.queue) : player.queue.map(Row.track)
    }

    var body: some View {
        VStack(spacing: 0) {
            header

            if player.queue.isEmpty {
                EmptyState(
                    icon: "list.bullet",
                    title: "Queue is empty",
                    detail: "Press ⌘K to find something to play."
                )
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollViewReader { scroll in
                    List(selection: $selection) {
                        ForEach(rows) { row in
                            rowView(row)
                        }
                        .onMove(perform: move)
                    }
                    .listStyle(.inset)
                    .washedGround()
                    // `g` / `G`. Watches the token rather than the edge: jumping
                    // to where you already are still has to scroll, because the
                    // list may have been moved since.
                    .onChange(of: ui.queueJumpToken) { _, _ in
                        jump(to: ui.queueJumpTarget, using: scroll)
                    }
                    // Double-click and context menu both come from the List, keyed
                    // on the rows under the pointer rather than on a gesture.
                    .contextMenu(forSelectionType: String.self) { ids in
                        menu(forRows: ids)
                    } primaryAction: { ids in
                        play(rowIds: ids)
                    }
                    // Enter plays the selection, the way Return opens things
                    // everywhere else on the platform.
                    .onKeyPress(.return) {
                        playSelection()
                        return .handled
                    }
                    .onDeleteCommand { removeSelected() }
                    // Mirror the *queue item* ids, not the row ids: an album
                    // heading's id is synthetic, and handing that to the engine
                    // gets it rejected as not being a queue item.
                    .onChange(of: selection) { _, new in
                        player.queueSelection = Set(itemIds(in: new))
                    }
                    .onChange(of: ui.selectAllToken) { _, _ in
                        guard onStage else { return }
                        selection = Set(rows.map(\.id))
                    }
                    .clearsSelection($selection)
                }
            }
        }
        // On the whole stage, not the List: an empty queue is exactly when you
        // want to drop a folder on it, and it has no rows to land on.
        .dropDestination(for: URL.self) { urls, _ in
            player.importFiles(urls) { library.libraryChanged() }
            return true
        }
    }

    // MARK: - Header

    private var header: some View {
        HStack(spacing: 12) {
            // What the queue *is*, when it is still something. A queue that
            // came from a playlist and has not been touched since follows that
            // playlist, and saying so is what makes the following legible: you
            // can see why an edit over there moved something here, and you can
            // see the moment it stops.
            switch playlists.lockedTo {
            case .playlist(let playlist):
                PlaylistArtwork(
                    sources: playlists.covers[playlist.id] ?? [],
                    cornerRadius: 4
                )
                .frame(width: 34, height: 34)
                .shadow(color: .black.opacity(0.25), radius: 3, y: 1)
            case .album(let album):
                AlbumArtwork(source: .album(album.id), size: .thumb, cornerRadius: 4)
                    .frame(width: 34, height: 34)
                    .shadow(color: .black.opacity(0.25), radius: 3, y: 1)
            case nil:
                EmptyView()
            }

            VStack(alignment: .leading, spacing: 1) {
                if let name = lockedName {
                    Text("Playing \(name)")
                        .font(.headline)
                        .lineLimit(1)
                } else {
                    Text("Queue")
                        .font(.headline)
                }
                Text(summary)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            Spacer()

            if !selection.isEmpty {
                Text("\(selectedItemIds.count) selected")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Button("Clear") { selection = [] }
                Button("Remove", role: .destructive) { removeSelected() }
            }

            // Beside the layout picker because both are about what you are
            // looking at rather than what is in the queue. Disabled rather than
            // hidden when nothing is playing: a control that comes and goes is
            // one you have to look for.
            Button { ui.jumpQueue(to: .playing) } label: {
                Image(systemName: Icon.jumpToPlaying)
            }
            .disabled(player.currentItemId == nil)
            .help("Scroll to what's playing")

            // Both modes shown with the active one lit, the way Finder switches
            // view. A single icon has to choose between naming the mode you are
            // in and the mode you would get, and whichever it picks the other
            // reading is available and wrong.
            Picker("Queue layout", selection: $grouped) {
                Image(systemName: Icon.album).tag(true)
                Image(systemName: Icon.queueSection).tag(false)
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .fixedSize()
            .help("Group by album, or one row per track")

            Button { player.undo() } label: { Image(systemName: Icon.undo) }
                .help("Undo (⌘Z)")
            Button { player.redo() } label: { Image(systemName: Icon.redo) }
                .help("Redo (⇧⌘Z)")

            Menu {
                Button {
                    playlists.naming = player.queue.compactMap(\.trackId)
                } label: {
                    Label("Save as Playlist…", systemImage: Icon.playlist)
                }
                Divider()
                Button(role: .destructive) { player.clearQueue() } label: {
                    Label("Clear Queue", systemImage: Icon.clear)
                }
            } label: {
                Image(systemName: "ellipsis.circle")
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .frame(width: 22)
        }
        .buttonStyle(.borderless)
        .padding(.horizontal, 16)
        .padding(.vertical, 11)
    }

    /// What the queue is, when it is still something someone chose.
    private var lockedName: String? {
        switch playlists.lockedTo {
        case .playlist(let playlist): playlist.name
        case .album(let album): album.title
        case nil: nil
        }
    }

    private var summary: String {
        let total = player.queue.compactMap(\.durationMs).reduce(0, +)
        let count = Format.count(Int64(player.queue.count), "track")
        return total > 0 ? "\(count) · \(Format.duration(total))" : count
    }

    /// Extracted because the type checker gives up on a switch this size
    /// inline in a ForEach.
    @ViewBuilder
    private func rowView(_ row: Row) -> some View {
        switch row {
        case .album(_, let group):
            QueueAlbumHeader(group: group)
                // No playable: a queue album is a run of queue items, not a
                // library album, so its actions are its own.
                .rowBehaviour()
        case .track(let item):
            QueueRow(
                item: QueueRowContent(item: item),
                isCurrent: item.queueItemId == player.currentItemId,
                isSelected: selection.contains(item.queueItemId),
                // Ungrouped there is no heading above to say what record this
                // is, so the row says it itself.
                showArtist: !grouped || item.artist != item.albumArtist,
                artwork: !grouped
            )
            .rowBehaviour()
        }
    }

    @ViewBuilder
    private func albumMenu(_ group: QueueGroup) -> some View {
        Button {
            if let first = group.items.first { player.play(itemId: first.queueItemId) }
        } label: {
            Label("Play", systemImage: Icon.play)
        }
        Button {
            player.remove(itemIds: group.items.map(\.queueItemId))
        } label: {
            Label("Remove Album", systemImage: Icon.remove)
        }
        Divider()
        AddToPlaylistMenu { $0(group.items.compactMap(\.trackId)) }
        Divider()
        organizeButton(trackIds: group.items.compactMap(\.trackId), title: group.album)
        if let trackId = group.items.compactMap(\.trackId).first {
            Button { showInLibrary(trackId: trackId, highlight: false) } label: {
                Label("Go to Album", systemImage: Icon.album)
            }
        }
        Button {
            Share.link(
                trackIds: group.items.compactMap(\.trackId),
                named: "\(group.albumArtist) — \(group.album)",
                engine: library.engine,
                player: player
            )
        } label: {
            Label("Copy Album Share Link", systemImage: Icon.share)
        }
    }

    /// Find the album a queue item came from, then go there.
    ///
    /// Resolved when the button is pressed, not while the menu is built:
    /// SwiftUI builds context menus as it builds rows, so a lookup here ran a
    /// blocking query per row and froze the window on a large queue.
    private func showInLibrary(trackId: Int64, highlight: Bool) {
        let engine = library.engine
        Task {
            let albumId = (try? await engine.track(trackId: trackId))??.albumId
            guard let albumId else {
                player.report("That track is no longer in the library.")
                return
            }
            nav.open(album: albumId, highlighting: highlight ? trackId : nil)
        }
    }

    @ViewBuilder
    private func trackMenu(_ item: QueueItem) -> some View {
        Button { player.play(itemId: item.queueItemId) } label: {
            Label("Play", systemImage: Icon.play)
        }
        Button { player.remove(itemIds: [item.queueItemId]) } label: {
            Label("Remove", systemImage: Icon.remove)
        }
        if let trackId = item.trackId {
            Divider()
            AddToPlaylistMenu { $0([trackId]) }
            Divider()
            organizeButton(trackIds: [trackId], title: item.title)
            let favourited = library.isFavourite(track: trackId)
            Button { library.toggleFavourite(track: trackId) } label: {
                Label(
                    favourited ? "Remove Favourite" : "Favourite Track",
                    systemImage: favourited ? Icon.favourited : Icon.favourite
                )
            }
            Button { showInLibrary(trackId: trackId, highlight: true) } label: {
                Label("Go to Album", systemImage: Icon.album)
            }
            Button {
                Share.link(
                    trackIds: [trackId],
                    named: item.title,
                    engine: library.engine,
                    player: player
                )
            } label: {
                Label("Copy Share Link", systemImage: Icon.share)
            }
        }
    }

    // MARK: - Selection

    /// Queue items behind the selection. Selecting an album heading means its
    /// whole run, which is the point of the heading being selectable.
    private var selectedItemIds: [String] { itemIds(in: selection) }

    /// Expand a set of row ids to the queue items they stand for. An album
    /// heading stands for its whole run; a track stands for itself.
    private func itemIds(in rowIds: Set<String>) -> [String] {
        rows.filter { rowIds.contains($0.id) }.flatMap(\.itemIds)
    }

    private func removeSelected() {
        player.remove(itemIds: selectedItemIds)
        selection = []
    }

    /// Play the first queue item the given rows stand for — the track itself,
    /// or the first track of the album whose heading was double-clicked.
    private func play(rowIds: Set<String>) {
        guard let id = itemIds(in: rowIds).first else { return }
        player.play(itemId: id)
    }

    private func playSelection() { play(rowIds: selection) }

    /// Scrolls only. The TUI's `g` moves a cursor because the cursor is how you
    /// look around there; here the pointer and the selection are separate things
    /// and moving the selection would throw away what you had picked.
    ///
    /// The playing row is centred rather than put at the top: what is playing
    /// is read against what comes after it, and a row at the top edge has no
    /// after.
    private func jump(to target: UIState.Jump, using scroll: ScrollViewProxy) {
        let row: String? = switch target {
        case .top: rows.first?.id
        case .bottom: rows.last?.id
        // A track row's id *is* its queue item's, in either layout.
        case .playing: player.currentItemId
        }
        guard let row else { return }
        let anchor: UnitPoint = switch target {
        case .top: .top
        case .bottom: .bottom
        case .playing: .center
        }
        withAnimation(.easeOut(duration: 0.18)) {
            scroll.scrollTo(row, anchor: anchor)
        }
    }

    /// The menu for whatever is under the pointer. An album heading gets the
    /// album's actions; anything else gets the track's.
    @ViewBuilder
    private func menu(forRows ids: Set<String>) -> some View {
        if ids.count == 1, let row = rows.first(where: { ids.contains($0.id) }) {
            switch row {
            case .album(_, let group): albumMenu(group)
            case .track(let item): trackMenu(item)
            }
        } else {
            Button { player.remove(itemIds: itemIds(in: ids)) } label: {
                Label("Remove", systemImage: Icon.remove)
            }
            Divider()
            AddToPlaylistMenu { $0(trackIds(in: ids)) }
            Divider()
            organizeButton(trackIds: trackIds(in: ids), title: nil)
        }
    }

    /// Library track IDs behind a set of rows. A queue item with no row — a
    /// file whose import failed — has no metadata to build a path from.
    private func trackIds(in rowIds: Set<String>) -> [Int64] {
        let wanted = Set(itemIds(in: rowIds))
        return player.queue.filter { wanted.contains($0.queueItemId) }.compactMap(\.trackId)
    }

    /// `title` names the one thing being organized; a multi-selection has no
    /// name, so it is described by its size instead.
    @ViewBuilder
    private func organizeButton(trackIds: [Int64], title: String?) -> some View {
        Button {
            // The window opens first: `begin` reads the config and resolves the
            // selection, and waiting on that would leave the click dead.
            openWindow(id: OrganizeWindow.id)
            Task {
                await organize.begin(
                    title: title ?? Format.count(Int64(trackIds.count), "track"),
                    trackIds: trackIds
                )
            }
        } label: {
            Label("Organize Files…", systemImage: Icon.organize)
        }
        .disabled(trackIds.isEmpty)
    }

    // MARK: - Reordering

    /// Moving a heading moves its whole album, which is why headings are rows.
    ///
    /// Anchors to the row being dropped *onto* and inserts before it, rather
    /// than to the row above and inserting after. The latter has no way to
    /// express "at the very top", and lands a drop on an album heading below
    /// that album's first track instead of above the album.
    private func move(from source: IndexSet, to destination: Int) {
        // Ordered, not a Set: these keep their relative order in the queue, and
        // dragging an album must not scramble its tracks.
        let moving = source.sorted().flatMap { rows[$0].itemIds }
        let movingSet = Set(moving)
        guard !moving.isEmpty else { return }

        // The first row at or after the drop that isn't itself being moved.
        if let target = rows[min(destination, rows.count)...]
            .first(where: { row in !row.itemIds.contains(where: movingSet.contains) })?
            .itemIds.first
        {
            player.move(itemIds: moving, target: target, after: false)
            return
        }

        // Nothing below the drop stays put: this is a move to the end.
        guard let last = player.queue.last(where: { !movingSet.contains($0.queueItemId) }) else {
            return
        }
        player.move(itemIds: moving, target: last.queueItemId, after: true)
    }
}

// MARK: - Rows

extension QueueView {
    /// A queue row: either an album heading or one track.
    enum Row: Identifiable {
        case album(id: String, group: QueueGroup)
        case track(QueueItem)

        var id: String {
            switch self {
            case .album(let id, _): id
            case .track(let item): item.queueItemId
            }
        }

        /// The queue items this row stands for.
        var itemIds: [String] {
            switch self {
            case .album(_, let group): group.items.map(\.queueItemId)
            case .track(let item): [item.queueItemId]
            }
        }

        /// Contiguous runs, mirroring the TUI: queue order is the user's, and
        /// collapsing two separate visits to the same record into one heading
        /// would misrepresent it. A heading precedes each run; tracks with no
        /// album stand alone.
        static func build(from queue: [QueueItem]) -> [Row] {
            var rows: [Row] = []
            var index = 0
            while index < queue.count {
                let first = queue[index]
                guard !first.album.isEmpty else {
                    rows.append(.track(first))
                    index += 1
                    continue
                }
                let run = queue[index...].prefix {
                    $0.album == first.album && $0.albumArtist == first.albumArtist
                }
                rows.append(.album(
                    id: "album:\(first.queueItemId)",
                    group: QueueGroup(
                        id: first.queueItemId,
                        albumArtist: first.albumArtist,
                        album: first.album,
                        items: Array(run)
                    )
                ))
                rows.append(contentsOf: run.map { Row.track($0) })
                index += run.count
            }
            return rows
        }
    }
}

// MARK: - Grouping

extension QueueItem {
    /// Which sleeve to draw for this row.
    ///
    /// The record wherever the library still knows it, so a queued album is one
    /// fetch and one cached bitmap rather than one of each per track on it.
    /// Anything the library has lost falls back to its own file.
    var sleeve: AlbumArtwork.Source? {
        if let albumId { return .album(albumId) }
        return trackId.map { .track($0) }
    }
}

struct QueueGroup: Identifiable {
    let id: String
    let albumArtist: String
    let album: String
    var items: [QueueItem]

    /// Contiguous runs of the same album, mirroring the TUI's grouping. Items
    /// with no album title each stand alone rather than collecting into an
    /// "unknown album" bucket that doesn't exist.
    static func group(_ items: [QueueItem]) -> [QueueGroup] {
        var groups: [QueueGroup] = []
        for item in items {
            let key = item.album
            if !key.isEmpty,
               var last = groups.last,
               last.album == key,
               last.albumArtist == item.albumArtist {
                last.items.append(item)
                groups[groups.count - 1] = last
            } else {
                groups.append(QueueGroup(
                    id: item.queueItemId,
                    albumArtist: item.albumArtist,
                    album: key,
                    items: [item]
                ))
            }
        }
        return groups
    }

    var year: String? { items.first?.year }
}

private struct QueueAlbumHeader: View {
    let group: QueueGroup

    var body: some View {
        HStack(spacing: 12) {
            // No tap-to-view here, unlike the album page: this cover sits in a
            // selectable, draggable row, and a tap gesture on it would eat the
            // click that selects the row.
            if let sleeve = group.items.first?.sleeve {
                AlbumArtwork(source: sleeve, size: .thumb, cornerRadius: 5)
                    .frame(width: 52, height: 52)
                    .shadow(color: .black.opacity(0.28), radius: 4, y: 2)
            }

            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(.primary)
                    .lineLimit(1)

                // Only when the line above is the record: a group with no album
                // title already leads with the artist.
                if !group.album.isEmpty {
                    Text(group.albumArtist.isEmpty ? "Unknown Artist" : group.albumArtist)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }

                Text(detail)
                    .font(.caption2.monospacedDigit())
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
            }

            Spacer()
        }
        .textCase(nil)
        .padding(.vertical, 6)
    }

    private var title: String {
        if !group.album.isEmpty { return group.album }
        return group.albumArtist.isEmpty ? "Unknown Artist" : group.albumArtist
    }

    /// "2007 · 11 tracks · 59:10 · FLAC". The codec only earns its place when
    /// the whole run shares one — a mixed group would be lying.
    private var detail: String {
        var parts: [String] = []
        if let year = group.year, !year.isEmpty { parts.append(year) }
        parts.append(Format.count(Int64(group.items.count), "track"))
        let total = group.items.compactMap(\.durationMs).reduce(0, +)
        if total > 0 { parts.append(Format.duration(total)) }
        let codecs = Set(group.items.compactMap(\.codec))
        if let codec = codecs.first, codecs.count == 1 { parts.append(codec.uppercased()) }
        return parts.joined(separator: " · ")
    }
}
