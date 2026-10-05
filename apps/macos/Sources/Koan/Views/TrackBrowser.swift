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
    @State private var selection: Set<Int64> = []
    #if os(macOS)
    @Environment(EngineMirror.self) private var mirror
    @Environment(CoverArtCache.self) private var art
    @Environment(PlayingLevels.self) private var levels
    @Environment(TransferMeter.self) private var meter
    @Environment(UIState.self) private var ui
    @Environment(\.roomTint) private var tint
    @Environment(\.onStage) private var onStage
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @AppStorage("graphics") private var graphics = Graphics.full
    #endif

    private var tracks: [Track] { library.visibleTracks }

    var body: some View {
        VStack(spacing: 0) {
            #if os(macOS)
            header
                .padding(.horizontal, 24)
                .padding(.top, 18)
                .padding(.bottom, 16)
            #endif

            if tracks.isEmpty {
                EmptyState(
                    icon: Icon.track,
                    title: library.isNarrowed ? "No matches" : "No tracks yet",
                    detail: library.isNarrowed ? nil : "Add a folder or sign in to a server in Settings."
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
    private var header: some View {
        VStack(alignment: .leading, spacing: 1) {
            Text("Tracks")
                .font(.title2.weight(.semibold))
            Text(count)
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// A `KoanTable` — see there for why the Mac's lists are AppKit.
    private var table: some View {
        let lines = tracks.enumerated().map { TrackLine(id: $1.id, kind: .track($1), lead: "\($0 + 1)", position: $0) }
        let queued = mirror.queuedByTrack
        let current = player.currentTrackId
        let playing = player.isPlaying
        let live = onStage && !reduceMotion && graphics.animatesIndicators
        let key: [AnyHashable] = [
            AnyHashable(current), AnyHashable(playing), AnyHashable(live), AnyHashable(tint),
            AnyHashable(library.favouriteTrackIds),
            AnyHashable(queued.map { "\($0.key):\($0.value.status)" }.sorted()),
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
                menu: { ids, environment in hostedMenu(menu(for: ids), environment: environment) },
                primaryAction: play,
                drag: { ids in tracks.filter { ids.contains($0.id) }.map { PlayableTransfer(.track($0)) } },
                selectAllToken: ui.selectAllToken,
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
                .primaryTap { play([track.id]) }
            }
        }
        .listStyle(.inset)
        .washedGround()
        .clearsSelection($selection)
        .contextMenu(forSelectionType: Int64.self) { ids in
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
