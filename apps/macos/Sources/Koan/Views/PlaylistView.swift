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

    /// Selection is local `@State` for the same reason the queue's is: reading
    /// an observable in `body` invalidates the whole List under the click that
    /// caused it.
    @State private var selection: Set<String> = []
    @State private var renaming = false
    @State private var renameTo = ""

    private var playlist: Playlist? { playlists.playlist(id: playlistId) }
    private var tracks: [Track] { playlists.tracks }

    /// A playlist can hold the same track twice, so a row's identity is its
    /// position, not its track id. Selecting one copy must not light the other.
    private var rows: [Row] {
        playlist?.grouped == true ? Row.build(from: tracks) : tracks.enumerated().map(Row.track)
    }

    var body: some View {
        VStack(spacing: 0) {
            header
                .padding(.horizontal, 24)
                .padding(.top, 18)
                .padding(.bottom, 16)

            if tracks.isEmpty {
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
                    .onMove(perform: move)
                }
                .listStyle(.inset)
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

    private var header: some View {
        HStack(alignment: .top, spacing: 16) {
            PlaylistArtwork(sources: playlists.covers[playlistId] ?? [])
                .frame(width: 108, height: 108)
                .shadow(color: .black.opacity(0.28), radius: 6, y: 3)

            VStack(alignment: .leading, spacing: 6) {
                Text(playlist?.name ?? "Playlist")
                    .font(.title2.weight(.semibold))
                    .lineLimit(2)
                Text(summary)
                    .font(.caption)
                    .foregroundStyle(.secondary)

                HStack(spacing: 10) {
                    Button { playAll() } label: {
                        Label("Play", systemImage: "play.fill")
                    }
                    Button { playAll(shuffled: true) } label: {
                        Label("Shuffle", systemImage: "shuffle")
                    }
                    Button { player.enqueue(trackIds: tracks.map(\.id)) } label: {
                        Label("Queue", systemImage: "text.append")
                    }
                }
                .disabled(tracks.isEmpty)
                .padding(.top, 2)
            }

            Spacer()

            if !selection.isEmpty {
                VStack(alignment: .trailing, spacing: 4) {
                    Text("\(positions(in: selection).count) selected")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    HStack(spacing: 8) {
                        Button("Remove", role: .destructive) { removeSelected() }
                        Button("Clear") { selection = [] }
                    }
                }
            }

            VStack(alignment: .trailing, spacing: 10) {
                // Both modes shown with the active one lit, the way the queue
                // does it: a single icon has to choose between naming the mode
                // you are in and the mode you would get.
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
                    Button("Shuffle Playlist") { playlists.shuffle(id: playlistId) }
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
        .buttonStyle(.borderless)
    }

    /// The remembered choice, defaulting to ungrouped.
    private var groupedBinding: Binding<Bool> {
        Binding(
            get: { playlist?.grouped ?? false },
            set: { playlists.setGrouped($0, for: playlistId) }
        )
    }

    private var summary: String {
        var parts = [Format.count(Int64(tracks.count), "track")]
        let total = tracks.compactMap(\.durationMs).reduce(0, +)
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
        case .track(let position, let track):
            PlaylistTrackRow(
                track: track,
                position: position + 1,
                isCurrent: player.currentTrackId == track.id,
                isSelected: selection.contains(row.id),
                showsAlbum: playlist?.grouped != true
            )
            .rowBehaviour(playable: .track(track))
        }
    }

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
        rows.filter { rowIds.contains($0.id) }.flatMap(\.positions)
    }

    private func removeSelected() {
        remove(positions: positions(in: selection))
        selection = []
    }

    private func remove(positions doomed: [Int]) {
        guard !doomed.isEmpty else { return }
        let dropping = Set(doomed)
        let kept = tracks.enumerated()
            .filter { !dropping.contains($0.offset) }
            .map(\.element.id)
        playlists.setTracks(kept, in: playlistId)
    }

    @ViewBuilder
    private func menu(forRows ids: Set<String>) -> some View {
        let chosen = positions(in: ids)
        Button("Play") { play(rowIds: ids) }
        Button("Play Next") { player.playNext(trackIds: trackIds(at: chosen)) }
        Button("Add to Queue") { player.enqueue(trackIds: trackIds(at: chosen)) }
        Divider()
        Button("Remove from Playlist", role: .destructive) {
            remove(positions: chosen)
            selection = []
        }
        if chosen.count == 1, let track = tracks[safe: chosen[0]] {
            Divider()
            Button(library.isFavourite(track: track.id) ? "Remove Favourite" : "Favourite Track") {
                library.toggleFavourite(track: track.id)
            }
            if let albumId = track.albumId {
                Button("Go to Album") { nav.open(album: albumId, highlighting: track.id) }
            }
        }
    }

    private func trackIds(at positions: [Int]) -> [Int64] {
        positions.sorted().compactMap { tracks[safe: $0]?.id }
    }

    // MARK: - Reordering

    /// Moving a heading moves its whole album, which is why headings are rows.
    ///
    /// Positions rather than ids throughout: a playlist can hold the same track
    /// twice, and moving "that track" would be ambiguous.
    private func move(from source: IndexSet, to destination: Int) {
        let moving = source.sorted().flatMap { rows[$0].positions }
        guard !moving.isEmpty else { return }
        let movingSet = Set(moving)

        // Anchor on the first row at or after the drop that isn't itself
        // moving. Anchoring on the row above instead has no way to express "at
        // the very top".
        let anchor = rows[min(destination, rows.count)...]
            .first { row in !row.positions.contains(where: movingSet.contains) }?
            .positions.first

        var kept = tracks.enumerated()
            .filter { !movingSet.contains($0.offset) }
            .map { ($0.offset, $0.element.id) }
        let lifted = moving.map { tracks[$0].id }

        let insertAt = anchor.flatMap { position in
            kept.firstIndex { $0.0 == position }
        } ?? kept.count
        kept.insert(contentsOf: lifted.map { (-1, $0) }, at: insertAt)

        playlists.setTracks(kept.map(\.1), in: playlistId)
    }
}

// MARK: - Rows

extension PlaylistView {
    /// A playlist row: an album heading, or one track at one position.
    enum Row: Identifiable {
        case album(id: String, group: PlaylistGroup)
        case track(position: Int, track: Track)

        /// Keyed on position, not on track id — the same track can be in the
        /// playlist twice and the two copies are different rows.
        var id: String {
            switch self {
            case .album(let id, _): id
            case .track(let position, _): "track:\(position)"
            }
        }

        var positions: [Int] {
            switch self {
            case .album(_, let group): group.positions
            case .track(let position, _): [position]
            }
        }

        /// Contiguous runs of the same record, mirroring the queue's grouping.
        /// Playlist order is the user's, so two separate visits to a record are
        /// two headings rather than one.
        static func build(from tracks: [Track]) -> [Row] {
            var rows: [Row] = []
            var index = 0
            while index < tracks.count {
                let first = tracks[index]
                guard !first.albumTitle.isEmpty else {
                    rows.append(.track(position: index, track: first))
                    index += 1
                    continue
                }
                let run = tracks[index...].prefix { $0.albumTitle == first.albumTitle }
                rows.append(.album(
                    id: "album:\(index)",
                    group: PlaylistGroup(
                        album: first.albumTitle,
                        artist: first.artistName,
                        positions: Array(index..<(index + run.count)),
                        tracks: Array(run)
                    )
                ))
                rows.append(contentsOf: run.enumerated().map { offset, track in
                    Row.track(position: index + offset, track: track)
                })
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
    let tracks: [Track]
}

private struct PlaylistAlbumHeader: View {
    let group: PlaylistGroup

    var body: some View {
        HStack(spacing: 12) {
            if let trackId = group.tracks.first?.id {
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

private struct PlaylistTrackRow: View {
    let track: Track
    let position: Int
    let isCurrent: Bool
    let isSelected: Bool
    /// Ungrouped there is no heading above saying what record this is, so the
    /// row carries its own sleeve and names it.
    let showsAlbum: Bool

    @Environment(PlayerModel.self) private var player
    @Environment(LibraryModel.self) private var library
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 10) {
            if showsAlbum {
                AlbumArtwork(source: .track(track.id), cornerRadius: 3)
                    .frame(width: 28, height: 28)
            } else {
                Text("\(position)")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.tertiary)
                    .frame(width: 24, alignment: .trailing)
            }

            VStack(alignment: .leading, spacing: 1) {
                Text(track.title)
                    .lineLimit(1)
                    .foregroundStyle(
                        isCurrent && !isSelected
                            ? AnyShapeStyle(.tint)
                            : AnyShapeStyle(.primary)
                    )
                Text(showsAlbum ? "\(track.artistName) — \(track.albumTitle)" : track.artistName)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }

            Spacer(minLength: 8)

            // Playback state is mirrored from the queue, not owned here: a row
            // is lit because that track is what is playing.
            if isCurrent {
                PlayingIndicator(isPlaying: player.isPlaying)
                    .font(.caption)
            }

            FavouriteButton(
                isOn: library.isFavourite(track: track.id),
                showing: hovering,
                size: .caption
            ) {
                library.toggleFavourite(track: track.id)
            }
            .frame(width: 16)

            if let ms = track.durationMs {
                Text(Format.duration(ms))
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
                    .frame(width: 44, alignment: .trailing)
            }
        }
        .frame(height: showsAlbum ? 40 : 34)
        // The same six points the queue gives a row carrying its own sleeve,
        // and the album heading gives its cover.
        .padding(.vertical, showsAlbum ? 6 : 0)
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
    }
}

extension Array {
    /// Index that answers `nil` rather than trapping. Positions here come from
    /// a row list built off a track list that may have been reloaded since.
    subscript(safe index: Int) -> Element? {
        indices.contains(index) ? self[index] : nil
    }
}
