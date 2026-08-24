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
    @Environment(LibraryModel.self) private var library
    @Environment(OrganizeModel.self) private var organize
    @Environment(\.openWindow) private var openWindow
    @Environment(UIState.self) private var ui

    @State private var savingSnapshot = false
    @State private var snapshotName = ""

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
    private var rows: [Row] { Row.build(from: player.queue) }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()

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
                    // `g` / `G`. Watches the token rather than the edge: jumping
                    // to where you already are still has to scroll, because the
                    // list may have been moved since.
                    .onChange(of: ui.queueJumpToken) { _, _ in
                        jump(to: ui.queueJumpEdge, using: scroll)
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
                    .onChange(of: player.selectAllToken) { _, _ in
                        selection = Set(rows.map(\.id))
                    }
                }
            }
        }
        // On the whole stage, not the List: an empty queue is exactly when you
        // want to drop a folder on it, and it has no rows to land on.
        .dropDestination(for: URL.self) { urls, _ in
            player.importFiles(urls) { library.libraryChanged() }
            return true
        }
        .alert("Save Queue", isPresented: $savingSnapshot) {
            TextField("Name", text: $snapshotName)
            Button("Cancel", role: .cancel) { snapshotName = "" }
            Button("Save") {
                guard !snapshotName.isEmpty else { return }
                library.saveSnapshot(name: snapshotName)
                snapshotName = ""
            }
        } message: {
            Text("Stores the queue and playback position under a name you can restore later.")
        }
    }

    // MARK: - Header

    private var header: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 1) {
                Text("Queue")
                    .font(.headline)
                Text(summary)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            Spacer()

            if !selection.isEmpty {
                Text("\(selectedItemIds.count) selected")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Button("Remove", role: .destructive) { removeSelected() }
            }

            Button { player.undo() } label: { Image(systemName: "arrow.uturn.backward") }
                .help("Undo (⌘Z)")
            Button { player.redo() } label: { Image(systemName: "arrow.uturn.forward") }
                .help("Redo (⇧⌘Z)")

            Menu {
                Button("Save as Snapshot…") { savingSnapshot = true }
                Divider()
                Button("Clear Queue", role: .destructive) { player.clearQueue() }
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
                item: item,
                isCurrent: item.queueItemId == player.currentItemId,
                isSelected: selection.contains(item.queueItemId),
                showArtist: item.artist != item.albumArtist
            )
            .rowBehaviour()
        }
    }

    @ViewBuilder
    private func albumMenu(_ group: QueueGroup) -> some View {
        Button("Play") {
            if let first = group.items.first { player.play(itemId: first.queueItemId) }
        }
        Button("Remove Album") {
            player.remove(itemIds: group.items.map(\.queueItemId))
        }
        Divider()
        organizeButton(trackIds: group.items.compactMap(\.trackId), title: group.album)
        if let trackId = group.items.compactMap(\.trackId).first {
            Button("Go to Album") { showInLibrary(trackId: trackId, highlight: false) }
        }
        Button("Copy Album Share Link") {
            Share.link(
                trackIds: group.items.compactMap(\.trackId),
                named: "\(group.albumArtist) — \(group.album)",
                engine: library.engine,
                player: player
            )
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
            library.reveal(album: albumId, highlighting: highlight ? trackId : nil)
        }
    }

    @ViewBuilder
    private func trackMenu(_ item: QueueItem) -> some View {
        Button("Play") { player.play(itemId: item.queueItemId) }
        Button("Remove") { player.remove(itemIds: [item.queueItemId]) }
        if let trackId = item.trackId {
            Divider()
            organizeButton(trackIds: [trackId], title: item.title)
            Button(library.isFavourite(track: trackId) ? "Remove Favourite" : "Favourite Track") {
                library.toggleFavourite(track: trackId)
            }
            Button("Go to Album") { showInLibrary(trackId: trackId, highlight: true) }
            Button("Copy Share Link") {
                Share.link(
                    trackIds: [trackId],
                    named: item.title,
                    engine: library.engine,
                    player: player
                )
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
    private func jump(to edge: UIState.Edge, using scroll: ScrollViewProxy) {
        guard let target = edge == .top ? rows.first?.id : rows.last?.id else { return }
        withAnimation(.easeOut(duration: 0.18)) {
            scroll.scrollTo(target, anchor: edge == .top ? .top : .bottom)
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
            Button("Remove") { player.remove(itemIds: itemIds(in: ids)) }
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
        Button("Organize Files…") {
            // The window opens first: `begin` reads the config and resolves the
            // selection, and waiting on that would leave the click dead.
            openWindow(id: OrganizeWindow.id)
            Task {
                await organize.begin(
                    title: title ?? Format.count(Int64(trackIds.count), "track"),
                    trackIds: trackIds
                )
            }
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
        HStack(spacing: 9) {
            if let trackId = group.items.first?.trackId {
                AlbumArtwork(source: .track(trackId), cornerRadius: 3)
                    .frame(width: 22, height: 22)
            }
            Text(group.albumArtist.isEmpty ? "Unknown Artist" : group.albumArtist)
                .foregroundStyle(.primary)
            if !group.album.isEmpty {
                Text("—")
                    .foregroundStyle(.tertiary)
                Text(group.album)
                    .foregroundStyle(.secondary)
            }
            if let year = group.year, !year.isEmpty {
                Text(year)
                    .foregroundStyle(.tertiary)
            }
            Spacer()
        }
        .font(.caption.weight(.medium))
        .textCase(nil)
        .padding(.vertical, 3)
    }
}

private struct QueueRow: View {
    let item: QueueItem
    let isCurrent: Bool
    let isSelected: Bool
    let showArtist: Bool

    @Environment(PlayerModel.self) private var player
    @Environment(LibraryModel.self) private var library
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 10) {
            statusIcon
                .font(.caption)
                .frame(width: 16)

            // Always occupies its column, number or not: a missing track
            // number would otherwise shift the title left and break the
            // alignment down the list.
            Text(item.trackNumber.map(String.init) ?? "")
                .font(.caption.monospacedDigit())
                .foregroundStyle(.tertiary)
                .frame(width: 20, alignment: .trailing)

            VStack(alignment: .leading, spacing: 1) {
                Text(item.title)
                    .lineLimit(1)
                    // Tinted to mark the playing track — but not when the row
                    // is selected, where accent-on-accent is unreadable.
                    .foregroundStyle(
                        isCurrent && !isSelected
                            ? AnyShapeStyle(.tint)
                            : AnyShapeStyle(.primary)
                    )
                // Only worth a second line when it differs from the album
                // artist — compilations and features, not every track.
                if showArtist && !item.artist.isEmpty {
                    Text(item.artist)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            }

            Spacer(minLength: 8)

            if let progress = item.downloadProgress {
                ProgressView(value: progress)
                    .progressViewStyle(.linear)
                    .frame(width: 54, height: 12)
            }

            if let trackId = item.trackId {
                FavouriteButton(
                    isOn: library.isFavourite(track: trackId),
                    showing: hovering,
                    size: .caption
                ) {
                    library.toggleFavourite(track: trackId)
                }
                .frame(width: 16)
            } else {
                // Keeps the column even for an item with no library row, so
                // the durations stay in line down the queue.
                Color.clear.frame(width: 16, height: 1)
            }

            if let codec = item.codec {
                Text(codec.uppercased())
                    .font(.caption2.monospaced())
                    .foregroundStyle(.tertiary)
            }

            if let ms = item.durationMs {
                Text(Format.duration(ms))
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
                    .frame(width: 44, alignment: .trailing)
            }
        }
        // Fixed height so a row doesn't grow when a download indicator appears
        // and shrink when it finishes, reflowing the list each time.
        .frame(height: 34)
        // Without this the row is only clickable where a view actually sits —
        // the Spacer between the title and the duration is a dead zone, and
        // clicks landing there select nothing.
        .contentShape(Rectangle())
        .opacity(item.status == .played ? 0.45 : 1)
        .onHover { hovering = $0 }
    }

    @ViewBuilder
    private var statusIcon: some View {
        switch item.status {
        case .playing:
            Image(systemName: player.isPlaying ? "speaker.wave.2.fill" : "speaker.fill")
                .foregroundStyle(.tint)
        case .downloading:
            Image(systemName: "arrow.down.circle").foregroundStyle(.secondary)
        case .priorityPending:
            Image(systemName: "arrow.down.circle.fill").foregroundStyle(.tint)
        case .failed:
            Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange)
        case .played:
            Image(systemName: "checkmark").foregroundStyle(.tertiary)
        case .queued:
            Image(systemName: "circle.dotted").foregroundStyle(.quaternary)
        }
    }
}
