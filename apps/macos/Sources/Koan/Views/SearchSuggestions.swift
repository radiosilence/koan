import KoanFFI
import SwiftUI

/// The dropdown under the search field: a few of each kind, enough to
/// recognise what you meant. Everything else lives behind Return.
struct SearchSuggestions: View {
    @Environment(SearchModel.self) private var search
    @Environment(LibraryModel.self) private var library

    var body: some View {
        if !search.quickTracks.isEmpty {
            Section("Tracks") {
                ForEach(search.quickTracks, id: \.id) { track in
                    Button {
                        // Take me to it, don't play it — you play from the
                        // album page, the same as if you'd browsed there.
                        if let albumId = track.albumId {
                            library.reveal(album: albumId, highlighting: track.id)
                        }
                        search.reset()
                    } label: {
                        SuggestionRow(
                            icon: "music.note",
                            title: track.title,
                            subtitle: "\(track.artistName) — \(track.albumTitle)"
                        )
                    }
                }
            }
        }

        if !search.quickAlbums.isEmpty {
            Section("Albums") {
                ForEach(search.quickAlbums, id: \.id) { album in
                    Button {
                        library.reveal(album: album.id)
                        search.reset()
                    } label: {
                        SuggestionRow(
                            icon: "square.stack",
                            title: album.title,
                            subtitle: album.artistName
                        )
                    }
                }
            }
        }

        if !search.quickArtists.isEmpty {
            Section("Artists") {
                ForEach(search.quickArtists, id: \.id) { artist in
                    Button {
                        library.reveal(artist: artist.id)
                        search.reset()
                    } label: {
                        SuggestionRow(icon: "music.mic", title: artist.name, subtitle: nil)
                    }
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
