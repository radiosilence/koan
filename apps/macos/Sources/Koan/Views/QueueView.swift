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

    @State private var selection: Set<String> = []
    @State private var savingSnapshot = false
    @State private var snapshotName = ""

    private var groups: [QueueGroup] { QueueGroup.group(player.queue) }

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
                    List(selection: $selection) {
                        ForEach(groups) { group in
                            Section {
                                ForEach(group.items, id: \.queueItemId) { item in
                                    QueueRow(
                                        item: item,
                                        isCurrent: item.queueItemId == player.nowPlaying.queueItemId,
                                        showArtist: item.artist != group.albumArtist
                                    )
                                    .id(item.queueItemId)
                                    .onTapGesture(count: 2) { player.play(itemId: item.queueItemId) }
                                    .contextMenu { menu(for: item) }
                                }
                                .onMove { move(in: group, from: $0, to: $1) }
                            } header: {
                                QueueAlbumHeader(group: group)
                            }
                        }
                    }
                    .listStyle(.inset)
                    .onDeleteCommand { removeSelected() }
                    // Follow the cursor as tracks advance, but never fight a
                    // user who has scrolled somewhere deliberately.
                    .onChange(of: player.nowPlaying.queueItemId) { _, id in
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

    /// `onMove` indices are relative to the group; the engine wants item IDs
    /// against the whole queue. Translate through the group's own offsets.
    private func move(in group: QueueGroup, from source: IndexSet, to destination: Int) {
        let moving = source.map { group.items[$0].queueItemId }
        let anchorIndex = destination - 1
        guard group.items.indices.contains(anchorIndex) else {
            // Dropped above the group's first row — anchor to whatever precedes it.
            guard let first = group.items.first,
                  let globalIndex = player.queue.firstIndex(where: { $0.queueItemId == first.queueItemId }),
                  globalIndex > 0
            else { return }
            player.move(itemIds: moving, after: player.queue[globalIndex - 1].queueItemId)
            return
        }
        let anchor = group.items[anchorIndex].queueItemId
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
                    .frame(width: 54)
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
        .padding(.vertical, 2)
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
