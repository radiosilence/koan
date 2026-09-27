import KoanFFI
import SwiftUI

/// A playlist as a row: its mosaic, its name, how many tracks.
///
/// Shared by the Mac's sidebar and the phone's playlists tab.
struct PlaylistRow: View {
    let playlist: Playlist
    let covers: [AlbumArtwork.Source]

    var body: some View {
        HStack(spacing: 8) {
            PlaylistArtwork(sources: covers, cornerRadius: 3)
                .frame(width: 24, height: 24)

            VStack(alignment: .leading, spacing: 0) {
                Text(playlist.name)
                    .lineLimit(1)
                Text(Format.count(Int64(playlist.trackCount), "track"))
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
        }
        // Full width, so the drop target is the row rather than the text — the
        // same reason the Queue row does it.
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
    }
}
