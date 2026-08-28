import KoanFFI
import SwiftUI

/// Everything you have favourited, in one page.
///
/// koan favourites artists and records as well as tracks, and this page only
/// ever showed the tracks — the other two were invisible from inside the app.
/// Sections rather than a type picker, for the same reason search results are
/// sections: they are all answers to one question, and a mode you have to
/// remember you are in is a worse way to find out you favourited a record.
///
/// One `List` rather than search's `ScrollView`, because the tracks here are a
/// working list: range-select, Return to play, a menu on the selection. The
/// artists and records ride above them as rows that cannot be selected.
struct FavouritesView: View {
    @Environment(PlayerModel.self) private var player
    @Environment(Navigator.self) private var nav
    @Environment(LibraryModel.self) private var library

    @State private var selection: Set<Int64> = []

    private let columns = [GridItem(.adaptive(minimum: 140, maximum: 190), spacing: 16)]

    private var artists: [Artist] { library.visibleFavouriteArtists }
    private var albums: [Album] { library.visibleFavouriteAlbums }
    private var tracks: [Track] { library.visibleFavourites }

    var body: some View {
        VStack(spacing: 0) {
            header
                .padding(.horizontal, 24)
                .padding(.top, 18)
                .padding(.bottom, 16)

            if artists.isEmpty && albums.isEmpty && tracks.isEmpty {
                EmptyState(
                    icon: "heart",
                    title: library.filter.isEmpty ? "Nothing favourited yet" : "No matches",
                    detail: library.filter.isEmpty
                        ? "Hit the heart on a track, a record or an artist."
                        : nil
                )
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                List(selection: $selection) {
                    if !artists.isEmpty { artistSection }
                    if !albums.isEmpty { albumSection }
                    if !tracks.isEmpty { trackSection }
                }
                .listStyle(.inset)
                .washedGround()
                .clearsSelection($selection)
                .contextMenu(forSelectionType: Int64.self) { ids in
                    menu(for: ids)
                } primaryAction: { ids in
                    play(ids)
                }
                .onKeyPress(.return) {
                    play(selection)
                    return KeyPress.Result.handled
                }
            }
        }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 1) {
            Text("Favourites")
                .font(.title2.weight(.semibold))
            Text(summary)
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// Only the kinds you actually have, so a tracks-only library reads exactly
    /// as it did before there were three of them.
    private var summary: String {
        var parts: [String] = []
        if !artists.isEmpty { parts.append(Format.count(Int64(artists.count), "artist")) }
        if !albums.isEmpty { parts.append(Format.count(Int64(albums.count), "album")) }
        if !tracks.isEmpty { parts.append(Format.count(Int64(tracks.count), "track")) }
        return parts.joined(separator: " · ")
    }

    // MARK: - Sections

    private var artistSection: some View {
        Section("Artists") {
            FlowLayout(spacing: 8) {
                ForEach(artists, id: \.id) { artist in
                    ArtistPill(name: artist.name, artistId: artist.id)
                }
            }
            .padding(.vertical, 4)
            .selectionDisabled()
        }
    }

    private var albumSection: some View {
        Section("Albums") {
            LazyVGrid(columns: columns, spacing: 18) {
                ForEach(albums, id: \.id) { album in
                    AlbumGridCell(album: album)
                }
            }
            .padding(.vertical, 6)
            .selectionDisabled()
        }
    }

    private var trackSection: some View {
        Section("Tracks") {
            // Once per pass, not once per row — see `TrackListView`.
            let allTrackIds = tracks.map(\.id)
            ForEach(Array(tracks.enumerated()), id: \.element.id) { index, track in
                TrackRow(
                    track: track,
                    position: index + 1,
                    isCurrent: player.currentTrackId == track.id,
                    isSelected: selection.contains(track.id),
                    // Gathered from all over, so a row carries its own sleeve
                    // and says which record it came from.
                    showsAlbum: true,
                    allTrackIds: allTrackIds
                )
                .rowBehaviour(playable: .track(track))
            }
        }
    }

    // MARK: - Actions

    /// Plays from the first of `ids`, keeping the rest of the list behind it.
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
            Button("Play") {
                player.playNow(trackIds: chosen.map(\.id))
                nav.showQueueWhenReady(watching: player)
            }
            Button("Play Next") { player.playNext(trackIds: chosen.map(\.id)) }
            Button("Add to Queue") { player.enqueue(trackIds: chosen.map(\.id)) }
        }
    }
}
