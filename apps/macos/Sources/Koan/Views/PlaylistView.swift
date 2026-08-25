import KoanFFI
import SwiftUI

/// A playlist, laid out the way the queue is.
///
/// Almost everything the queue does applies here — album headings over
/// contiguous runs, multi-select, drag to reorder, ⌫ to remove, drop to add —
/// because a playlist and a queue are the same shape of thing. The differences
/// are the two that matter: this list outlives the session, and playback state
/// is *mirrored* onto it rather than owned by it. A row is lit because that
/// track is what is playing, the same way it is on an album page.
///
/// Ungrouped by default, unlike the queue: a playlist is a sequence someone
/// chose, not a shelf of records. The choice is remembered per playlist.
struct PlaylistView: View {
    let playlistId: Int64

    @Environment(PlayerModel.self) private var player
    @Environment(PlaylistsModel.self) private var playlists
    @Environment(LibraryModel.self) private var library
    @Environment(Navigator.self) private var nav
    @Environment(UIState.self) private var ui

    /// Selection is local `@State` for the same reason the queue's is: reading
    /// an observable in `body` invalidates the whole List under the click that
    /// caused it.
    @State private var selection: Set<String> = []
    @State private var renaming = false
    @State private var renameTo = ""

    private var playlist: Playlist? { playlists.playlist(id: playlistId) }
    private var entries: [PlaylistEntry] { playlists.entries }

    /// A playlist can hold the same track twice, so a row's identity is its
    /// position, not its track id. Selecting one copy must not light the other.
    private var rows: [Row] {
        grouped ? Row.build(from: entries) : entries.map(Row.entry)
    }

    var body: some View {
        VStack(spacing: 0) {
            header
                .padding(.horizontal, 24)
                .padding(.top, 18)
                .padding(.bottom, 16)

            if entries.isEmpty {
                EmptyState(
                    icon: "music.note.list",
                    title: playlists.isLoading ? "Loading…" : "Nothing in here yet",
                    detail: "Drag records, artists or tracks onto it — or onto its row in the sidebar."
                )
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                List(selection: $selection) {
                    ForEach(rows) { row in
                        rowView(row)
                    }
                }
                .listStyle(.inset)
                // Otherwise the List paints its own ground over the wash and
                // the record's colour stops in a hard line under the header,
                // rather than fading out across the first few rows. The queue
                // and every album page give theirs up for the same reason.
                .scrollContentBackground(.hidden)
                .contextMenu(forSelectionType: String.self) { ids in
                    menu(forRows: ids)
                } primaryAction: { ids in
                    play(rowIds: ids)
                }
                .onKeyPress(.return) {
                    play(rowIds: selection)
                    return .handled
                }
                .onDeleteCommand { removeSelected() }
                .clearsSelection($selection)
                .onChange(of: ui.selectAllToken) { _, _ in
                    selection = Set(rows.map(\.id))
                }
            }
        }
        // On the whole page, not the List: an empty playlist is exactly when
        // you want to drop something on it, and it has no rows to land on.
        .dropDestination(for: PlayableTransfer.self) { dropped, _ in
            playlists.add(dropped: dropped, to: playlistId)
            return true
        }
        .task(id: playlistId) {
            selection = []
            playlists.open(id: playlistId)
        }
        .alert("Rename Playlist", isPresented: $renaming) {
            TextField("Name", text: $renameTo)
            Button("Cancel", role: .cancel) {}
            Button("Rename") { playlists.rename(id: playlistId, to: renameTo) }
        }
    }

    // MARK: - Header

    /// A playlist is playable like a record is, so it gets the record's header:
    /// the big round play button beside the title, Play Next and Queue under
    /// it. It had three text buttons in a row, which is a shape nothing else in
    /// the app uses.
    private var playable: Playable? {
        playlist.map { .playlist(id: $0.id, name: $0.name) }
    }

    private var header: some View {
        HStack(alignment: .bottom, spacing: 18) {
            PlaylistArtwork(sources: playlists.covers[playlistId] ?? [], cornerRadius: 8)
                .frame(width: 132, height: 132)
                .shadow(color: .black.opacity(0.3), radius: 10, y: 4)

            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 12) {
                    if let playable {
                        PlayableHeaderButton(playable: playable)
                    }
                    Text(playlist?.name ?? "Playlist")
                        .font(.system(size: 26, weight: .semibold))
                        .lineLimit(2)
                }
                Text(summary)
                    .font(.callout)
                    .foregroundStyle(.secondary)

                HStack(spacing: 10) {
                    QueueButtons(playable: playable)
                    Button {
                        playlists.shuffle(id: playlistId)
                    } label: {
                        Label("Shuffle", systemImage: "shuffle")
                    }
                    .help("Reorder the playlist itself, for good")
                    .disabled(entries.count < 2)
                }
                .padding(.top, 4)
            }

            Spacer(minLength: 0)

            VStack(alignment: .trailing, spacing: 10) {
                if !selection.isEmpty {
                    HStack(spacing: 8) {
                        Text("\(positions(in: selection).count) selected")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        Button("Remove", role: .destructive) { removeSelected() }
                        Button("Clear") { selection = [] }
                    }
                    .buttonStyle(.borderless)
                }

                HStack(spacing: 10) {
                    // Both modes shown with the active one lit, the way the
                    // queue does it: a single icon has to choose between naming
                    // the mode you are in and the mode you would get.
                    Picker("Playlist layout", selection: groupedBinding) {
                        Image(systemName: "square.stack").tag(true)
                        Image(systemName: "list.bullet").tag(false)
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    .fixedSize()
                    .help("Group by album, or one row per track — remembered for this playlist")

                    Menu {
                        Button("Rename…") {
                            renameTo = playlist?.name ?? ""
                            renaming = true
                        }
                        Divider()
                        Button("Delete Playlist", role: .destructive) {
                            playlists.delete(id: playlistId)
                            nav.forget(.playlist(playlistId))
                            nav.show(.queue)
                        }
                    } label: {
                        Image(systemName: "ellipsis.circle")
                    }
                    .menuStyle(.borderlessButton)
                    .menuIndicator(.hidden)
                    .frame(width: 22)
                }
            }
        }
    }

    /// The remembered choice, defaulting to ungrouped.
    private var groupedBinding: Binding<Bool> {
        Binding(
            get: { grouped },
            set: { playlists.setGrouped($0, for: playlistId) }
        )
    }

    private var summary: String {
        var parts = [Format.count(Int64(entries.count), "track")]
        let total = entries.compactMap(\.track.durationMs).reduce(0, +)
        if total > 0 { parts.append(Format.duration(total)) }
        // Worth saying: it means edits made here show up on the server, and
        // edits made there show up here.
        if playlist?.remoteId != nil { parts.append("synced") }
        return parts.joined(separator: " · ")
    }

    // MARK: - Rows

    @ViewBuilder
    private func rowView(_ row: Row) -> some View {
        switch row {
        case .album(_, let group):
            PlaylistAlbumHeader(group: group)
                .rowBehaviour()
                .dropDestination(for: PlayableTransfer.self) { dropped, _ in
                    accept(dropped, before: group.positions.first ?? 0)
                }
        case .entry(let entry):
            let position = entries.firstIndex { $0.id == entry.id } ?? 0
            QueueRow(
                item: QueueRowContent(
                    entry: entry,
                    position: position + 1,
                    // Found by entry, not by track: two copies of one song are
                    // two rows, and each wears its own queue item's state.
                    queued: player.queuedByPlaylistEntry[entry.id],
                    isCurrent: player.currentPlaylistEntryId == entry.id
                ),
                isCurrent: player.currentPlaylistEntryId == entry.id,
                isSelected: selection.contains(row.id),
                // Ungrouped there is no heading above to say what record this
                // is, so the row says it itself.
                showArtist: true,
                artwork: !grouped
            )
            .rowBehaviour()
            // Carries where it came from, so dropping it back into this
            // playlist is a move of *this* row rather than of its track — and
            // dropping it anywhere else is just a track.
            .draggable(PlayableTransfer(
                kind: .track,
                id: entry.track.id,
                name: entry.track.title,
                origin: .init(playlistId: playlistId, position: position)
            ))
            .dropDestination(for: PlayableTransfer.self) { dropped, _ in
                accept(dropped, before: position)
            }
        }
    }

    private var grouped: Bool { playlist?.grouped ?? false }

    // MARK: - Actions

    private func playAll(shuffled: Bool = false) {
        start(at: nil, shuffled: shuffled)
    }

    /// Play from a position, keeping the rest of the playlist behind it — the
    /// same thing clicking track nine of an album does.
    ///
    /// Stays put afterwards: the playing row is lit on this page, so there is
    /// nothing the queue would show that this does not.
    private func start(at position: Int?, shuffled: Bool = false) {
        let engine = playlists.engine
        Task {
            _ = try? await engine.playPlaylist(
                playlistId: playlistId,
                startAt: position.map(UInt32.init),
                shuffled: shuffled
            )
        }
    }

    private func play(rowIds: Set<String>) {
        guard let first = positions(in: rowIds).min() else { return }
        start(at: first)
    }

    /// Expand a set of row ids to the playlist positions they stand for. An
    /// album heading stands for its whole run.
    private func positions(in rowIds: Set<String>) -> [Int] {
        rows.filter { rowIds.contains($0.id) }.flatMap { $0.positions(in: entries) }
    }

    private func entryIds(in rowIds: Set<String>) -> [Int64] {
        positions(in: rowIds).sorted().compactMap { entries[safe: $0]?.id }
    }

    private func trackIds(in rowIds: Set<String>) -> [Int64] {
        positions(in: rowIds).sorted().compactMap { entries[safe: $0]?.track.id }
    }

    private func removeSelected() {
        playlists.remove(entryIds: entryIds(in: selection), from: playlistId)
        selection = []
    }

    @ViewBuilder
    private func menu(forRows ids: Set<String>) -> some View {
        Button { play(rowIds: ids) } label: {
            Label("Play", systemImage: Icon.play)
        }
        Button { player.playNext(trackIds: trackIds(in: ids)) } label: {
            Label("Play Next", systemImage: Icon.playNext)
        }
        Button { player.enqueue(trackIds: trackIds(in: ids)) } label: {
            Label("Add to Queue", systemImage: Icon.queue)
        }
        Divider()
        AddToPlaylistMenu { $0(trackIds(in: ids)) }
        Divider()
        Button(role: .destructive) {
            playlists.remove(entryIds: entryIds(in: ids), from: playlistId)
            selection = []
        } label: {
            Label("Remove from Playlist", systemImage: Icon.remove)
        }
        if ids.count == 1, let position = positions(in: ids).first,
           let entry = entries[safe: position] {
            Divider()
            Button {
                library.toggleFavourite(track: entry.track.id)
            } label: {
                Label(
                    library.isFavourite(track: entry.track.id)
                        ? "Remove Favourite" : "Favourite Track",
                    systemImage: library.isFavourite(track: entry.track.id)
                        ? Icon.favourited : Icon.favourite
                )
            }
            if let albumId = entry.track.albumId {
                Button {
                    nav.open(album: albumId, highlighting: entry.track.id)
                } label: {
                    Label("Go to Album", systemImage: Icon.album)
                }
            }
        }
    }

    // MARK: - Reordering

    /// A drop landed on `position`: either rows of this playlist being moved,
    /// or tracks from elsewhere being added.
    ///
    /// Drop-based rather than `ForEach.onMove`, because `onMove` claims the
    /// drag — rows could be shuffled within the list but never dragged *out* of
    /// it, so a playlist could not feed the queue. The payload says where it
    /// came from, so one gesture covers both readings.
    @discardableResult
    private func accept(_ dropped: [PlayableTransfer], before position: Int) -> Bool {
        let mine = dropped.compactMap { $0.origin }.filter { $0.playlistId == playlistId }
        let foreign = dropped.filter { $0.origin?.playlistId != playlistId }

        if !mine.isEmpty {
            let moving = Set(mine.map(\.position))
            // The row dropped onto, by identity: its index shifts once the
            // moved rows are lifted out of the list.
            let anchor = entries[safe: position]?.id
            var order = entries.enumerated()
                .filter { !moving.contains($0.offset) }
                .map(\.element.id)
            let lifted = moving.sorted().compactMap { entries[safe: $0]?.id }
            let at = anchor.flatMap { order.firstIndex(of: $0) } ?? order.count
            order.insert(contentsOf: lifted, at: at)
            playlists.reorder(entryIds: order, in: playlistId)
        }

        if !foreign.isEmpty {
            // Appended rather than inserted at the drop: the engine adds to the
            // end, and a reorder straight after would race its reload.
            playlists.add(dropped: foreign, to: playlistId)
        }
        return true
    }
}

// MARK: - Rows

extension PlaylistView {
    /// A playlist row: an album heading, or one entry.
    enum Row: Identifiable {
        case album(id: String, group: PlaylistGroup)
        case entry(PlaylistEntry)

        /// The entry's own id. Two copies of one track are two entries and so
        /// two rows — selecting one must not light the other.
        var id: String {
            switch self {
            case .album(let id, _): id
            case .entry(let entry): "entry:\(entry.id)"
            }
        }

        /// Where this row sits. An album heading stands for its whole run; an
        /// entry finds itself by id, since its index moves as the list is
        /// edited underneath it.
        func positions(in entries: [PlaylistEntry]) -> [Int] {
            switch self {
            case .album(_, let group): group.positions
            case .entry(let entry):
                entries.firstIndex { $0.id == entry.id }.map { [$0] } ?? []
            }
        }

        /// Contiguous runs of the same record, mirroring the queue's grouping.
        /// Playlist order is the user's, so two separate visits to a record are
        /// two headings rather than one.
        static func build(from entries: [PlaylistEntry]) -> [Row] {
            var rows: [Row] = []
            var index = 0
            while index < entries.count {
                let first = entries[index]
                guard !first.track.albumTitle.isEmpty else {
                    rows.append(.entry(first))
                    index += 1
                    continue
                }
                let run = entries[index...].prefix {
                    $0.track.albumTitle == first.track.albumTitle
                }
                rows.append(.album(
                    id: "album:\(first.id)",
                    group: PlaylistGroup(
                        album: first.track.albumTitle,
                        artist: first.track.artistName,
                        positions: Array(index..<(index + run.count)),
                        entries: Array(run)
                    )
                ))
                rows.append(contentsOf: run.map { Row.entry($0) })
                index += run.count
            }
            return rows
        }
    }
}

/// A contiguous run of one record inside a playlist.
struct PlaylistGroup {
    let album: String
    let artist: String
    let positions: [Int]
    let entries: [PlaylistEntry]
}

private struct PlaylistAlbumHeader: View {
    let group: PlaylistGroup

    var body: some View {
        HStack(spacing: 12) {
            if let trackId = group.entries.first?.track.id {
                AlbumArtwork(source: .track(trackId), cornerRadius: 5)
                    .frame(width: 44, height: 44)
                    .shadow(color: .black.opacity(0.28), radius: 4, y: 2)
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(group.album)
                    .font(.system(size: 14, weight: .semibold))
                    .lineLimit(1)
                Text(group.artist.isEmpty ? "Unknown Artist" : group.artist)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer()
        }
        .textCase(nil)
        .padding(.vertical, 5)
    }
}

extension Array {
    /// Index that answers `nil` rather than trapping. Positions here come from
    /// a row list built off a track list that may have been reloaded since.
    subscript(safe index: Int) -> Element? {
        indices.contains(index) ? self[index] : nil
    }
}
