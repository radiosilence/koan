import KoanFFI
import SwiftUI

/// The dropdown under the search field: a few of each kind, enough to
/// recognise what you meant. Everything else lives behind Return.
///
/// Rows carry no actions, because suggestion rows can't — `searchSuggestions`
/// completes the *query text*. So each row completes to an opaque token
/// naming what was picked, and submit decodes it and routes there. Without
/// that the dropdown can only re-run the text as a search, which is how a
/// click on "Therapy Sessions" ends up on a list rather than the album.
struct SearchSuggestions: View {
    @Environment(SearchModel.self) private var search

    var body: some View {
        if !search.tracks.isEmpty {
            Section("Tracks") {
                ForEach(search.tracks.prefix(5), id: \.id) { track in
                    SuggestionRow(
                        icon: "music.note",
                        title: track.title,
                        subtitle: "\(track.artistName) — \(track.albumTitle)"
                    )
                    .searchCompletion(
                        SearchModel.Selection.track(track.id, album: track.albumId).token
                    )
                }
            }
        }

        if !search.albums.isEmpty {
            Section("Albums") {
                ForEach(search.albums.prefix(4), id: \.id) { album in
                    SuggestionRow(
                        icon: "square.stack",
                        title: album.title,
                        subtitle: album.artistName
                    )
                    .searchCompletion(SearchModel.Selection.album(album.id).token)
                }
            }
        }

        if !search.artists.isEmpty {
            Section("Artists") {
                ForEach(search.artists.prefix(4), id: \.id) { artist in
                    SuggestionRow(icon: "music.mic", title: artist.name, subtitle: nil)
                        .searchCompletion(SearchModel.Selection.artist(artist.id).token)
                }
            }
        }
    }
}

private struct SuggestionRow: View {
    let icon: String
    let title: String
    let subtitle: String?

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: icon)
                .foregroundStyle(.tertiary)
                .frame(width: 14)
            Text(title)
            if let subtitle {
                Text(subtitle)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
        }
    }
}
