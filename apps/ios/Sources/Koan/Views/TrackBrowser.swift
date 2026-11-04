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

    private var tracks: [Track] { library.visibleTracks }

    var body: some View {
        VStack(spacing: 0) {

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
                list
            }
        }
        #if os(iOS)
        .navigationSubtitle(count)
        #endif
    }

    private var count: String { Format.count(Int64(library.trackTotal), "track") }

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
