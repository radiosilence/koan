import KoanFFI
import SwiftUI

/// Every track in the library, narrowed by the browse filters and ordered by
/// the track sort: the third browser beside Albums and Artists, and where a
/// shelf's Tracks section goes to show all of them.
///
/// The listing arrives a page at a time — see `LibraryModel.continueTracks()`
/// — so the count in the header is the whole of it from the first page on,
/// and the rows fill in behind.
struct TrackBrowser: View {
    @Environment(LibraryModel.self) private var library
    @Environment(PlayerModel.self) private var player
    @Environment(Navigator.self) private var nav
    @Environment(EngineMirror.self) private var mirror
    @State private var selection: Set<Int64> = []
    #if os(iOS)
    @State private var editMode: EditMode = .inactive
    #endif
    #if os(macOS)
    @Environment(CoverArtCache.self) private var art
    @Environment(PlayingLevels.self) private var levels
    @Environment(TransferMeter.self) private var meter
    @Environment(UIState.self) private var ui
    @Environment(\.roomTint) private var tint
    @Environment(\.onStage) private var onStage
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    #endif

    private var tracks: [Track] { library.visibleTracks }

    #if os(macOS)
    /// The table's lines and the ids they hold, kept across the body runs a
    /// queue patch or a pause cause: rebuilt when the listing is replaced, and
    /// added to when a page lands on its end.
    @State private var listing = Listing()

    @MainActor
    final class Listing {
        private var revision: Int?
        private(set) var lines: [TrackLine] = []
        private(set) var ids: Set<Int64> = []

        func update(_ tracks: [Track], revision: Int) {
            if revision != self.revision || tracks.count < lines.count {
                self.revision = revision
                lines = []
                ids = []
            }
            guard tracks.count > lines.count else { return }
            for position in lines.count..<tracks.count {
                let track = tracks[position]
                lines.append(TrackLine(id: track.id, kind: .track(track), lead: "\(position + 1)", position: position))
                ids.insert(track.id)
            }
        }
    }
    #endif

    var body: some View {
        VStack(spacing: 0) {
            #if os(macOS)
            PageHeader("Tracks", appWords: true, detail: { Text(count) })
                .padding([.horizontal, .top], RowMetrics.pageEdge)
                .padding(.bottom, RowMetrics.headerGap)
            #endif

            if tracks.isEmpty {
                EmptyState(
                    icon: Icon.track,
                    title: library.isNarrowed ? "No matches" : "No tracks yet",
                    detail: library.isNarrowed
                        ? nil
                        : mirror.signInRefused
                        ? EngineMirror.signInRefusedDetail
                        : "Add a folder or sign in to a server in Settings."
                )
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                #if os(macOS)
                table
                #else
                list
                #endif
            }
        }
        #if os(iOS)
        .navigationSubtitle(count)
        #endif
    }

    private var count: String { Format.count(Int64(library.trackTotal), "track") }

    #if os(macOS)
    /// A `KoanTable` — see there for why the Mac's lists are AppKit.
    private var table: some View {
        listing.update(tracks, revision: library.tracksRevision)
        let lines = listing.lines
        let listed = listing.ids
        let queued = mirror.queuedByTrack
        let current = player.currentTrackId
        let playing = player.isPlaying
        let live = onStage && !reduceMotion
        let key: [AnyHashable] = [
            AnyHashable(current), AnyHashable(playing), AnyHashable(live), AnyHashable(tint),
            AnyHashable(library.favouriteTrackIds),
            // What the queue says of the tracks listed here, and nothing it
            // says of others: a patch to a row elsewhere redraws nothing.
            // Read from whichever side is shorter.
            lines.count <= queued.count
                ? AnyHashable(lines.map { queued[$0.id].map { "\($0.status)" } })
                : AnyHashable(queued.compactMap { listed.contains($0.key) ? "\($0.key):\($0.value.status)" : nil }.sorted()),
        ]
        let library = library
        let nav = nav
        return SafeAreaReader { insets in
            KoanTable(
                items: lines,
                id: \.id,
                context: TrackTableRow.Context(
                    showsAlbum: true,
                    leadWidth: TrackTableRow.leadWidth(for: tracks.count),
                    currentTrackId: current,
                    isPlaying: playing,
                    barsLive: live,
                    tint: NSColor(tint),
                    favourites: library.favouriteTrackIds,
                    queued: queued,
                    meter: meter,
                    art: art,
                    levels: levels,
                    play: { line in play([line.id]) },
                    openArtist: { nav.open(artist: $0) },
                    openAlbum: { nav.open(album: $0) },
                    toggleFavourite: { library.toggleFavourite(track: $0) }
                ),
                contextKey: AnyHashable(key),
                selection: $selection,
                make: TrackTableRow.init,
                rowHeight: TrackTableRow.artHeight,
                shape: AnyHashable([library.tracksRevision, lines.count]),
                menu: { ids, environment in hostedMenu(menu(for: ids), environment: environment) },
                primaryAction: play,
                drag: \.transfer,
                selectAllToken: ui.selectAllToken,
                offset: library.tracksOffset,
                noteOffset: { library.tracksOffset = $0 },
                complete: !library.pagingTracks,
                rewinds: nav.rewinds[.tracks] ?? 0,
                insets: insets
            )
        }
        .clearsSelection($selection)
    }
    #endif

    private var list: some View {
        List(selection: $selection) {
            // Once per pass, not once per row — see `TrackListView`.
            let allTrackIds = tracks.map(\.id)
            ForEach(Array(tracks.enumerated()), id: \.element.id) { index, track in
                TrackRow(
                    track: track,
                    position: index + 1,
                    showsAlbum: true,
                    allTrackIds: allTrackIds
                )
                .rowBehaviour(playable: .track(track))
                .primaryTap { play([track.id]) } menu: { menu(for: [track.id]) }
                .accessibilityIdentifier("track-\(track.id)")
            }
        }
        .insetList()
        .washedGround()
        #if os(iOS)
        .listSelectMode($editMode, selection: $selection) { TrackListView.selectionActions($0) }
        #endif
        .clearsSelection($selection)
        .selectionMenu(for: Int64.self) { ids in
            menu(for: ids)
        } primaryAction: { ids in
            play(ids)
        }
    }

    /// Plays from the first of `ids`, the rest of the listing behind it.
    private func play(_ ids: Set<Int64>) {
        guard let index = tracks.firstIndex(where: { ids.contains($0.id) }) else { return }
        player.playNow(trackIds: tracks.map(\.id), startingAt: index)
        nav.showQueueWhenReady(watching: player)
    }

    @ViewBuilder
    private func menu(for ids: Set<Int64>) -> some View {
        let chosen = tracks.filter { ids.contains($0.id) }
        if chosen.count == 1, let track = chosen.first {
            PlayableMenu(playable: .track(track))
        } else if !chosen.isEmpty {
            QueueActions(trackIds: chosen.map(\.id))
        }
    }
}
