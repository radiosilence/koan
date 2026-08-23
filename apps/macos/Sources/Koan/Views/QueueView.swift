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

    private var groups: [QueueGroup] { QueueGroup.group(player.queue) }

    /// Selection is local `@State`, deliberately.
    ///
    /// It lived on `PlayerModel` so the Edit menu could reach it, but reading an
    /// observable in the body means every selection change invalidates the whole
    /// view and rebuilds the List — under the very click that caused it, which
    /// is what made clicking here so unreliable. It is mirrored to the model on
    /// change instead: written, never read, so it stays out of the render path.
    @State private var selection: Set<String> = []

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
                ScrollViewReader { proxy in
                    // One flat ForEach, with album headings rendered inline
                    // rather than as Sections. `onMove` cannot cross a Section
                    // boundary, so grouping this way meant a track could only be
                    // dragged within its own album — which looked like "played
                    // items can't be dragged", they just tend to sit in an
                    // earlier group than the drop target.
                    List(selection: $selection) {
                        ForEach(Array(player.queue.enumerated()), id: \.element.queueItemId) { index, item in
                            VStack(alignment: .leading, spacing: 0) {
                                if let heading = heading(at: index) {
                                    QueueAlbumHeader(group: heading)
                                        .padding(.top, index == 0 ? 0 : 10)
                                        .padding(.bottom, 3)
                                        .contentShape(.rect)
                                        // Selects the whole run, so the next
                                        // drag moves the album rather than the
                                        // single row the heading sits above.
                                        .simultaneousGesture(TapGesture().onEnded {
                                            selection = Set(groupItemIds(from: index))
                                        })
                                        .help("Select this album")
                                }
                                QueueRow(
                                    item: item,
                                    isCurrent: item.queueItemId == player.currentItemId,
                                    showArtist: item.artist != item.albumArtist
                                )
                            }
                            .simultaneousGesture(TapGesture(count: 2).onEnded {
                                player.play(itemId: item.queueItemId)
                            })
                            .contextMenu { menu(for: item) }
                        }
                        .onMove(perform: move)
                    }
                    .listStyle(.inset)
                    .onDeleteCommand { removeSelected() }
                    // One-way mirror so the Edit menu can act on the selection
                    // without the render path having to observe it.
                    .onChange(of: selection) { _, new in player.queueSelection = new }
                    // Follow the cursor as tracks advance, but never fight a
                    // user who has scrolled somewhere deliberately.
                    .onChange(of: player.currentItemId) { _, id in
                        guard let id, selection.isEmpty else { return }
                        withAnimation { proxy.scrollTo(id, anchor: .center) }
                    }
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
                Text("\(selection.count) selected")
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

    // MARK: - Actions

    @ViewBuilder
    private func menu(for item: QueueItem) -> some View {
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

    private func removeSelected() {
        player.remove(itemIds: Array(selection))
        selection = []
    }

    /// Every queue item belonging to the run that starts at `index`.
    private func groupItemIds(from index: Int) -> [String] {
        let start = player.queue[index]
        return player.queue[index...]
            .prefix { $0.album == start.album && $0.albumArtist == start.albumArtist }
            .map(\.queueItemId)
    }

    /// The album heading to draw above this row, if it starts a new run.
    ///
    /// Contiguous runs, mirroring the TUI: queue order is the user's, and
    /// collapsing two separate visits to the same record into one heading would
    /// misrepresent it.
    private func heading(at index: Int) -> QueueGroup? {
        let item = player.queue[index]
        guard !item.album.isEmpty else { return nil }
        if index > 0 {
            let previous = player.queue[index - 1]
            guard previous.album != item.album || previous.albumArtist != item.albumArtist else {
                return nil
            }
        }
        return QueueGroup(
            id: item.queueItemId,
            albumArtist: item.albumArtist,
            album: item.album,
            items: [item]
        )
    }

    /// `onMove` speaks in indices over the whole queue; the engine speaks in
    /// item IDs. Translate at the boundary and let it decide the result.
    private func move(from source: IndexSet, to destination: Int) {
        let moving = source.map { player.queue[$0].queueItemId }
        guard destination > 0 else {
            // Dropped at the very top — anchor to the first item that isn't moving.
            guard let anchor = player.queue.first(where: { !moving.contains($0.queueItemId) })
            else { return }
            player.move(itemIds: moving, after: anchor.queueItemId)
            return
        }
        let anchor = player.queue[destination - 1].queueItemId
        guard !moving.contains(anchor) else { return }
        player.move(itemIds: moving, after: anchor)
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

            if let n = item.trackNumber {
                Text("\(n)")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.tertiary)
                    .frame(width: 20, alignment: .trailing)
            }

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
