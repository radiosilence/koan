import KoanFFI
import SwiftUI

/// Everything matching the query, in sections. Selecting a result takes you to
/// where it lives in the library rather than playing it — you play from the
/// album or artist page, the same way you would if you'd browsed there.
struct SearchResultsView: View {
    @Environment(SearchModel.self) private var search
    @Environment(LibraryModel.self) private var library

    private let columns = [GridItem(.adaptive(minimum: 140, maximum: 190), spacing: 16)]

    var body: some View {
        ScrollView {
            if !search.hasQuery {
                EmptyState(icon: "magnifyingglass", title: "Search your library")
                    .frame(maxWidth: .infinity, minHeight: 320)
            } else if search.isEmpty && !search.isSearching {
                EmptyState(
                    icon: "magnifyingglass",
                    title: "Nothing found",
                    detail: "No artists, albums or tracks match “\(search.query)”."
                )
                .frame(maxWidth: .infinity, minHeight: 320)
            } else {
                VStack(alignment: .leading, spacing: 26) {
                    if !search.artists.isEmpty { artistSection }
                    if !search.albums.isEmpty { albumSection }
                    if !search.tracks.isEmpty { trackSection }
                }
                .padding(22)
            }
        }
        .navigationTitle("Results for “\(search.query)”")
    }

    private var artistSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            SectionHeading("Artists", count: search.artists.count)
            FlowLayout(spacing: 8) {
                ForEach(search.artists, id: \.id) { artist in
                    ArtistPill(name: artist.name, artistId: artist.id)
                }
            }
        }
    }

    private var albumSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            SectionHeading("Albums", count: search.albums.count)
            LazyVGrid(columns: columns, spacing: 18) {
                ForEach(search.albums, id: \.id) { album in
                    AlbumGridCell(album: album)
                }
            }
        }
    }

    private var trackSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            SectionHeading("Tracks", count: search.tracks.count)
            VStack(spacing: 0) {
                ForEach(search.tracks, id: \.id) { track in
                    SearchTrackRow(track: track)
                }
            }
        }
    }
}

private struct SectionHeading: View {
    let title: String
    let count: Int

    init(_ title: String, count: Int) {
        self.title = title
        self.count = count
    }

    var body: some View {
        HStack(spacing: 7) {
            Text(title)
                .font(.title3.weight(.semibold))
            Text("\(count)")
                .font(.caption.monospacedDigit())
                .foregroundStyle(.tertiary)
        }
    }
}

/// A track result points at its album — that's "the place in the library" the
/// track lives, and where you'd play it from.
///
/// Behaves like any other row: click to go where it lives, drag it to enqueue,
/// right-click for the same menu the tiles and pills have. It was a `Button`,
/// which claims the press and left the row as the one result you could neither
/// drag nor right-click.
private struct SearchTrackRow: View {
    let track: Track

    @Environment(LibraryModel.self) private var library
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 10) {
            // The cover is what you recognise a track by, and a results
            // list is exactly where you are trying to recognise something.
            Group {
                if let albumId = track.albumId {
                    AlbumArtwork(source: .album(albumId), cornerRadius: 3)
                } else {
                    Image(systemName: "music.note")
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                }
            }
            .frame(width: 40, height: 40)

            VStack(alignment: .leading, spacing: 1) {
                Text(track.title).lineLimit(1)
                Text("\(track.artistName) — \(track.albumTitle)")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                SourceBadges(track: track)
            }

            Spacer(minLength: 8)

            if track.albumId != nil && hovering {
                Text("Go to album")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }

            Text(Format.duration(track.durationMs))
                .font(.caption.monospacedDigit())
                .foregroundStyle(.tertiary)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 5)
        .background {
            RoundedRectangle(cornerRadius: 6)
                .fill(hovering ? AnyShapeStyle(.quaternary.opacity(0.5)) : AnyShapeStyle(.clear))
        }
        .rowBehaviour(playable: .track(track))
        .onTapGesture {
            guard let albumId = track.albumId else { return }
            library.reveal(album: albumId, highlighting: track.id)
        }
        .contextMenu { PlayableMenu(playable: .track(track)) }
        .onHover { hovering = $0 }
    }
}
