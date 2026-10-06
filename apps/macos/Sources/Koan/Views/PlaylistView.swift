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

    @Environment(\.horizontalSizeClass) private var width
    @Environment(PlayerModel.self) private var player
    @Environment(PlaylistsModel.self) private var playlists
    @Environment(LibraryModel.self) private var library
    @Environment(Navigator.self) private var nav
    @Environment(UIState.self) private var ui
    #if os(macOS)
    @Environment(EngineMirror.self) private var mirror
    @Environment(CoverArtCache.self) private var art
    @Environment(PlayingLevels.self) private var levels
    @Environment(TransferMeter.self) private var meter
    @Environment(\.roomTint) private var tint
    @Environment(\.onStage) private var onStage
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @AppStorage("graphics") private var graphics = Graphics.full
    #endif

    /// Selection is local `@State` for the same reason the queue's is, and
    /// unread here for the same reason too — see `QueueView.selection`.
    @State private var selection: Set<String> = []
    /// Where a drop would land, so the gesture says what it will do. See
    /// `insertionLine(showing:)`.
    @State private var dropBefore: Int?
    @State private var renaming = false
    @State private var renameTo = ""

    private var playlist: Playlist? { playlists.playlist(id: playlistId) }
    /// Guarded on the id because history can move faster than a read.
    private var entries: [PlaylistEntry] {
        playlists.openId == playlistId ? playlists.entries : []
    }

    /// A playlist can hold the same track twice, so a row's identity is its
    /// position, not its track id. Selecting one copy must not light the other.
    private var rows: [Row] {
        grouped
            ? Row.build(from: entries)
            : entries.enumerated().map { Row.entry($1, position: $0) }
    }

    var body: some View {
        // Once per pass, for the header and the rows both.
        let rows = self.rows

        VStack(spacing: 0) {
            header(rows)
                .padding(.horizontal, 24)
                .padding(.top, 18)
                .padding(.bottom, 16)

            if entries.isEmpty {
                EmptyState(
                    icon: "music.note.list",
                    title: playlists.isLoading ? "Loading…" : "Nothing in here yet",
                    detail: playlist?.smart == true
                        ? "Nothing in the library matches its rules yet."
                        : playlist?.readonly == true
                        ? "None of its tracks are in the library."
                        : "Drag records, artists or tracks onto it — or onto its row in the sidebar."
                )
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                #if os(macOS)
                table(rows)
                #else
                List(selection: $selection) {
                    ForEach(rows) { row in
                        rowView(row)
                    }
                    endOfList
                }
                .insetList()
                .washedGround()
                .selectionMenu(for: String.self) { ids in
                    menu(forRows: ids)
                } primaryAction: { ids in
                    play(rowIds: ids)
                }
                .onKeyPress(.return) {
                    play(rowIds: selection)
                    return .handled
                }
                #if os(macOS)
                .onDeleteCommand { removeSelected() }
                #endif
                .clearsSelection($selection)
                .onChange(of: ui.selectAllToken) { _, _ in
                    selection = Set(rows.map(\.id))
                }
                #endif
            }
        }
        // On the whole page, not the List: an empty playlist is exactly when
        // you want to drop something on it, and it has no rows to land on.
        .dropTarget(for: PlayableTransfer.self) { dropped, _ in
            playlists.add(dropped: dropped, to: playlistId)
            return true
        }
        // Only for a library change — the rows arrived before the page did.
        .reloading(on: playlistId) {
            await playlists.prepare(id: playlistId)
        }
        .onChange(of: playlistId) { selection = [] }
        .alert("Rename Playlist", isPresented: $renaming) {
            TextField("Name", text: $renameTo)
            Button(KoanTheme.label("Cancel"), role: .cancel) {}
            Button(KoanTheme.label("Rename")) { playlists.rename(id: playlistId, to: renameTo) }
        }
    }

    #if os(macOS)
    /// A `KoanTable` — see there for why the Mac's lists are AppKit.
    private func table(_ rows: [Row]) -> some View {
        let current = player.currentPlaylistEntryId
        let lines = rows.map { row -> QueueLine in
            switch row {
            case .album(let id, let group):
                let track = group.entries.first?.track
                return QueueLine(id: id, kind: .heading(QueueHeading(
                    title: group.album,
                    artist: group.artist.isEmpty ? "Unknown Artist" : group.artist,
                    detail: nil,
                    sleeve: track.map { $0.albumId.map { .album($0) } ?? .track($0.id) },
                    sleeveSize: 44
                )))
            case .entry(let entry, let position):
                let isCurrent = current == entry.entryId
                return QueueLine(id: row.id, kind: .track(
                    QueueRowContent(
                        entry: entry,
                        position: position + 1,
                        queued: mirror.queuedByPlaylistEntry[entry.entryId],
                        isCurrent: isCurrent
                    ),
                    isCurrent: isCurrent,
                    showArtist: true,
                    artwork: !grouped
                ))
            }
        }
        let live = onStage && !reduceMotion && graphics.animatesIndicators
        let key: [AnyHashable] = [
            AnyHashable(player.isPlaying), AnyHashable(live), AnyHashable(onStage), AnyHashable(tint), AnyHashable(library.favouriteTrackIds),
        ]
        // Where a drop before the row at `index` lands in the playlist.
        let position = { (index: Int) -> Int in
            guard index < rows.count else { return entries.count }
            return rows[index].positions.first ?? entries.count
        }
        return SafeAreaReader { insets in
            KoanTable(
                items: lines,
                id: \.id,
                context: QueueTableRow.Context(
                    isPlaying: player.isPlaying,
                    barsLive: live,
                    tint: NSColor(tint),
                    favourites: library.favouriteTrackIds,
                    meter: meter,
                    onStage: onStage,
                    art: art,
                    levels: levels,
                    toggleFavourite: { library.toggleFavourite(track: $0) }
                ),
                contextKey: AnyHashable(key),
                selection: $selection,
                make: QueueTableRow.init,
                heightOf: QueueTableRow.height(of:),
                changed: { $0 != $1 },
                menu: { ids, environment in hostedMenu(menu(forRows: ids), environment: environment) },
                primaryAction: { play(rowIds: $0) },
                // Carries where it came from, so dropping it back into this
                // playlist is a move of *this* row rather than of its track —
                // and dropping it anywhere else is just a track.
                drag: { ids in
                    rows.filter { ids.contains($0.id) }.compactMap { row -> PlayableTransfer? in
                        guard case .entry(let entry, let at) = row else { return nil }
                        return PlayableTransfer(
                            kind: .track, id: entry.track.id, name: entry.track.title,
                            origin: .init(playlistId: playlistId, position: at)
                        )
                    }
                },
                delete: { _ in removeSelected() },
                selectAllToken: ui.selectAllToken,
                accept: { dropped, index in accept(dropped, before: position(index)) },
                insets: EdgeInsets(top: 0, leading: insets.leading, bottom: insets.bottom, trailing: 0)
            )
        }
        .clearsSelection($selection)
    }
    #endif

    // MARK: - Header

    /// A playlist is playable like a record is, so it gets the record's header:
    /// the big round play button beside the title, Play Next and Queue under
    /// it.
    private var playable: Playable? {
        playlist.map { .playlist(id: $0.id, name: $0.name) }
    }

    /// Side by side where there is room, stacked where there is not, as a
    /// record's page is: on a phone the side-by-side header is wider than the
    /// screen, which wraps the title and Shuffle a letter at a time and lays
    /// every row below out at the header's width, clipped.
    @ViewBuilder private func header(_ rows: [Row]) -> some View {
        if width == .compact {
            VStack(alignment: .leading, spacing: 14) {
                artwork
                titleBlock
                HStack(spacing: 10) {
                    QueueButtons(playable: playable)
                    shuffleButton.labelStyle(.iconOnly)
                    Spacer(minLength: 0)
                    layoutControls
                }
                PlaylistSelectionHeader(selection: $selection, rows: rows) { removeSelected() }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        } else {
            HStack(alignment: .bottom, spacing: 18) {
                artwork

                VStack(alignment: .leading, spacing: 6) {
                    titleBlock
                    HStack(spacing: 10) {
                        QueueButtons(playable: playable)
                        shuffleButton
                    }
                    .padding(.top, 4)
                }

                Spacer(minLength: 0)

                VStack(alignment: .trailing, spacing: 10) {
                    PlaylistSelectionHeader(selection: $selection, rows: rows) { removeSelected() }
                    layoutControls
                }
            }
        }
    }

    private var artwork: some View {
        PlaylistArtwork(sources: playlists.covers[playlistId] ?? [], cornerRadius: KoanTheme.radius(8))
            .frame(width: 132, height: 132)
            .shadow(color: .black.opacity(KoanTheme.isOn ? 0 : 0.3), radius: 10, y: 4)
    }

    private var titleBlock: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 12) {
                #if !os(tvOS)
                if let playable {
                    PlayableHeaderButton(playable: playable)
                }
                #endif
                Text(playlist?.name ?? "Playlist")
                    .font(.role(.title, system: .system(size: 26, weight: .semibold)))
                    .foregroundStyle(KoanTheme.style(.strong, system: .primary))
                    .lineLimit(2)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Text(summary)
                .font(.role(.control, system: .callout))
                .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
            #if os(tvOS)
            if let playable {
                PlayableHeaderButton(playable: playable)
                    .padding(.top, 12)
            }
            #endif
        }
    }

    private var shuffleButton: some View {
        Button {
            playlists.shuffle(id: playlistId)
        } label: {
            Label("Shuffle", systemImage: "shuffle")
        }
        .help("Reorder the playlist itself, for good")
        .disabled(entries.count < 2)
    }

    private var layoutControls: some View {
        HStack(spacing: 10) {
            // Both modes shown with the active one lit, the way the
            // queue does it: a single icon has to choose between naming
            // the mode you are in and the mode you would get.
            Picker("Playlist layout", selection: groupedBinding) {
                Image(systemName: Icon.album).tag(true)
                Image(systemName: Icon.queueSection).tag(false)
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .fixedSize()
            .help("Group by album, or one row per track — remembered for this playlist")

            Menu {
                Button(KoanTheme.label("Rename…")) {
                    renameTo = playlist?.name ?? ""
                    renaming = true
                }
                .disabled(playlist?.fromFile == true)
                Divider()
                Button(KoanTheme.label("Delete Playlist"), role: .destructive) {
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
            dropTarget(
                PlaylistAlbumHeader(group: group).rowBehaviour(),
                before: group.positions.first ?? 0
            )
        case .entry(let entry, let position):
            dropTarget(
                PlaylistEntryRow(entry: entry, position: position, artwork: !grouped)
                    .rowBehaviour()
                    .primaryTap { play(rowIds: [row.id]) } menu: { menu(forRows: [row.id]) }
                    // Carries where it came from, so dropping it back into this
                    // playlist is a move of *this* row rather than of its track —
                    // and dropping it anywhere else is just a track.
                    .dragSource(PlayableTransfer(
                        kind: .track,
                        id: entry.track.id,
                        name: entry.track.title,
                        origin: .init(playlistId: playlistId, position: position)
                    )),
                before: position
            )
        }
    }

    /// Somewhere to drop that means "after everything".
    ///
    /// Every other target is a row, and a row can only mean "before this one" —
    /// so without this there is no gesture for the end of the list, which is
    /// where most things are added.
    private var endOfList: some View {
        dropTarget(
            Color.clear
                .frame(height: 28)
                .rowSeparator(.hidden)
                .listRowBackground(Color.clear)
                .selectionDisabled(),
            before: entries.count
        )
    }

    /// A row that takes drops landing before `position`, with the line that
    /// says so.
    private func dropTarget(_ row: some View, before position: Int) -> some View {
        row
            .insertionLine(showing: dropBefore == position)
            .dropTarget(for: PlayableTransfer.self) { dropped, _ in
                dropBefore = nil
                return accept(dropped, before: position)
            } isTargeted: { targeted in
                dropBefore = targeted ? position : (dropBefore == position ? nil : dropBefore)
            }
    }

    private var grouped: Bool { playlist?.grouped ?? false }

    // MARK: - Actions

    /// Play from an entry, keeping the rest of the playlist behind it — the
    /// same thing clicking track nine of an album does. An entry rather than a
    /// position: see `playPlaylist`.
    ///
    /// Stays put afterwards: the playing row is lit on this page, so there is
    /// nothing the queue would show that this does not.
    private func start(at entry: Int64) {
        let engine = playlists.engine
        Task {
            _ = try? await engine.playPlaylist(
                playlistId: playlistId,
                startEntry: entry,
                shuffled: false
            )
        }
    }

    private func play(rowIds: Set<String>) {
        guard let first = positions(in: rowIds).min(), let entry = entries[safe: first] else {
            return
        }
        start(at: entry.entryId)
    }

    /// Expand a set of row ids to the playlist positions they stand for. An
    /// album heading stands for its whole run.
    private func positions(in rowIds: Set<String>) -> [Int] {
        Row.positions(in: rowIds, of: rows)
    }

    private func entryIds(in rowIds: Set<String>) -> [Int64] {
        positions(in: rowIds).sorted().compactMap { entries[safe: $0]?.entryId }
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
            let anchor = entries[safe: position]?.entryId
            var order = entries.enumerated()
                .filter { !moving.contains($0.offset) }
                .map(\.element.entryId)
            let lifted = moving.sorted().compactMap { entries[safe: $0]?.entryId }
            let at = anchor.flatMap { order.firstIndex(of: $0) } ?? order.count
            order.insert(contentsOf: lifted, at: at)
            playlists.reorder(entryIds: order, in: playlistId)
        }

        if !foreign.isEmpty {
            playlists.insert(dropped: foreign, into: playlistId, at: position)
        }
        return true
    }
}

// MARK: - Rows

extension PlaylistView {
    /// A playlist row: an album heading, or one entry.
    ///
    /// An entry carries its position rather than finding itself by id in the
    /// entries each time it is asked — a walk of the list per row per
    /// evaluation, on every click and every pause. The rows are rebuilt
    /// whenever the entries move, so the position cannot go stale.
    enum Row: Identifiable {
        case album(id: String, group: PlaylistGroup)
        case entry(PlaylistEntry, position: Int)

        /// The entry's own id. Two copies of one track are two entries and so
        /// two rows — selecting one must not light the other.
        var id: String {
            switch self {
            case .album(let id, _): id
            case .entry(let entry, _): "entry:\(entry.entryId)"
            }
        }

        /// Where this row sits. An album heading stands for its whole run.
        var positions: [Int] {
            switch self {
            case .album(_, let group): group.positions
            case .entry(_, let position): [position]
            }
        }

        /// Expand a set of row ids to the playlist positions they stand for.
        static func positions(in rowIds: Set<String>, of rows: [Row]) -> [Int] {
            rows.filter { rowIds.contains($0.id) }.flatMap(\.positions)
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
                    rows.append(.entry(first, position: index))
                    index += 1
                    continue
                }
                let run = entries[index...].prefix { sameRecord($0.track, first.track) }
                rows.append(.album(
                    id: "album:\(first.entryId)",
                    group: PlaylistGroup(
                        album: first.track.albumTitle,
                        artist: first.track.albumArtistName,
                        positions: Array(index..<(index + run.count)),
                        entries: Array(run)
                    )
                ))
                rows.append(contentsOf: run.enumerated().map { Row.entry($1, position: index + $0) })
                index += run.count
            }
            return rows
        }

        /// Two records can share a title; the id tells them apart where there is one.
        private static func sameRecord(_ a: Track, _ b: Track) -> Bool {
            if let id = a.albumId, let other = b.albumId { return id == other }
            return a.albumTitle == b.albumTitle && a.albumArtistName == b.albumArtistName
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

/// One entry, wearing whatever the queue currently thinks of it.
///
/// Its own view so that what is playing and what is queued are read per row:
/// read by the list, every pause and every queue edit would re-run all of it.
private struct PlaylistEntryRow: View {
    let entry: PlaylistEntry
    let position: Int
    /// Ungrouped there is no heading above to say what record this is, so the
    /// row says it itself.
    let artwork: Bool

    @Environment(PlayerModel.self) private var player
    @Environment(EngineMirror.self) private var mirror

    var body: some View {
        let isCurrent = player.currentPlaylistEntryId == entry.entryId
        QueueRow(
            item: QueueRowContent(
                entry: entry,
                position: position + 1,
                // Found by entry, not by track: two copies of one song are
                // two rows, and each wears its own queue item's state.
                queued: mirror.queuedByPlaylistEntry[entry.entryId],
                isCurrent: isCurrent
            ),
            isCurrent: isCurrent,
            showArtist: true,
            artwork: artwork
        )
    }
}

/// What the selection is, and what to do with it. The one reader of the
/// selection outside the List, so a click re-runs this and not the page.
private struct PlaylistSelectionHeader: View {
    @Binding var selection: Set<String>
    let rows: [PlaylistView.Row]
    let remove: () -> Void

    var body: some View {
        if !selection.isEmpty {
            HStack(spacing: 8) {
                Text("\(PlaylistView.Row.positions(in: selection, of: rows).count) selected")
                    .font(.role(.fine, system: .caption))
                    .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                Button(KoanTheme.label("Remove"), role: .destructive, action: remove)
                Button(KoanTheme.label("Clear")) { selection = [] }
            }
            .buttonStyle(.borderless)
        }
    }
}

private struct PlaylistAlbumHeader: View {
    let group: PlaylistGroup

    var body: some View {
        HStack(spacing: 12) {
            // By record where the library knows it: art is stored per record,
            // and asking by track fetches the same sleeve once per run.
            if let track = group.entries.first?.track {
                AlbumArtwork(
                    source: track.albumId.map { .album($0) } ?? .track(track.id), cornerRadius: KoanTheme.radius(5)
                )
                    .frame(width: 44, height: 44)
                    .shadow(color: .black.opacity(KoanTheme.isOn ? 0 : 0.28), radius: 4, y: 2)
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(group.album)
                    .font(.role(.body, system: .system(size: 14, weight: .semibold)))
                    .lineLimit(1)
                Text(group.artist.isEmpty ? "Unknown Artist" : group.artist)
                    .font(.role(.fine, system: .caption))
                    .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
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


private extension View {
    /// The line that says where a drop will land.
    ///
    /// Drawn on the row it would land *before*, which is what a drop on a row
    /// means here. `ForEach.onMove` would draw one of these, but it claims the
    /// drag, so nothing could be dragged out of the playlist.
    func insertionLine(showing: Bool) -> some View {
        overlay(alignment: .top) {
            if showing {
                Capsule()
                    .fill(.tint)
                    .frame(height: 2)
                    .transition(.opacity)
            }
        }
        .animation(.easeOut(duration: 0.12), value: showing)
    }
}
