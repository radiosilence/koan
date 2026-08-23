import KoanFFI
import SwiftUI

/// One album in a grid. Shared by the library browser and the artist page so
/// they behave identically — art plays the record, the title opens it, the
/// artist name links out.
struct AlbumGridCell: View {
    let album: Album
    /// An artist's own page already says whose records these are.
    var showArtist: Bool = true

    @Environment(PlayerModel.self) private var player
    @Environment(LibraryModel.self) private var library

    @State private var titleHovering = false

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            PlayableArtwork(albumId: album.id)
                .shadow(color: .black.opacity(0.28), radius: 7, y: 3)

            Text(album.title)
                .font(.callout.weight(.medium))
                .underline(titleHovering)
                .lineLimit(1)
                .contentShape(.rect)
                .onHover { titleHovering = $0 }
                .onTapGesture { library.reveal(album: album.id) }

            HStack(spacing: 4) {
                if showArtist {
                    LinkText(text: album.artistName, target: .artist(album.artistId), font: .caption)
                }
                if let year = album.year {
                    Text(showArtist ? "· \(String(year))" : String(year))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .contextMenu {
            Button("Play") { play(replacing: true) }
            Button("Add to Queue") { play(replacing: false) }
            Divider()
            Button("Go to Album") { library.reveal(album: album.id) }
            Button("Go to Artist") { library.reveal(artist: album.artistId) }
        }
    }

    /// Loading the tracks is the slow half, so it happens off the main actor and
    /// the queue command follows once they're in hand.
    private func play(replacing: Bool) {
        let engine = library.engine
        let albumId = album.id
        Task {
            let ids = await Task.detached(priority: .userInitiated) {
                ((try? engine.tracks(
                    albumId: albumId, artistId: nil, sort: .album, limit: 500, offset: 0
                )) ?? []).map(\.id)
            }.value
            if replacing { player.playNow(trackIds: ids) } else { player.enqueue(trackIds: ids) }
        }
    }
}
