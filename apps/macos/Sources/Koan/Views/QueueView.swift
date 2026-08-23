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
                List(selection: $selection) {
                    ForEach(rows) { row in
                        rowView(row)
                    }
                    .onMove(perform: move)
                }
                .listStyle(.inset)
                .onDeleteCommand { removeSelected() }
                .onChange(of: selection) { _, new in player.queueSelection = new }
                .onChange(of: player.selectAllToken) { _, _ in
                    selection = Set(rows.map(\.id))
                }
            }
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
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
                .contextMenu { albumMenu(group) }
                .onTapGesture(count: 2) {
                    if let first = group.items.first { player.play(itemId: first.queueItemId) }
                }
        case .track(let item):
            QueueRow(
                item: item,
                isCurrent: item.queueItemId == player.currentItemId,
                showArtist: item.artist != item.albumArtist
            )
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
            .contextMenu { trackMenu(item) }
            .onTapGesture(count: 2) { player.play(itemId: item.queueItemId) }
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
    }

    @ViewBuilder
    private func trackMenu(_ item: QueueItem) -> some View {
        Button("Play") { player.play(itemId: item.queueItemId) }
        Button("Remove") { player.remove(itemIds: [item.queueItemId]) }
        if let trackId = item.trackId {
            Divider()
            Button("Favourite") {
                player.toggleFavourite(trackId: trackId)
                library.refreshFavourites()
            }
        }
    }

    // MARK: - Selection

    /// Queue items behind the selection. Selecting an album heading means its
    /// whole run, which is the point of the heading being selectable.
    private var selectedItemIds: [String] {
        rows.filter { selection.contains($0.id) }.flatMap(\.itemIds)
    }

    private func removeSelected() {
        player.remove(itemIds: selectedItemIds)
        selection = []
    }

    /// Double-click plays what the first click selected — the track, or the
    /// first track of the album whose heading was clicked.
    private func playSelection() {
        guard let id = selectedItemIds.first else { return }
        player.play(itemId: id)
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
    let showArtist: Bool

    @Environment(PlayerModel.self) private var player

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
                    .foregroundStyle(isCurrent ? AnyShapeStyle(.tint) : AnyShapeStyle(.primary))
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
