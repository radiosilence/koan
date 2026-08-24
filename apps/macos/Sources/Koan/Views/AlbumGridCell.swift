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
    @Environment(Navigator.self) private var nav
    @Environment(LibraryModel.self) private var library

    @State private var titleHovering = false
    @State private var hovering = false

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            PlayableArtwork(albumId: album.id)
                .shadow(color: .black.opacity(0.28), radius: 7, y: 3)
                .overlay(alignment: .topTrailing) {
                    if let codec = album.codec {
                        // Format only, no sample rate: at tile size the rate is
                        // unreadable and it's the codec that tells you whether
                        // this is the good copy.
                        Text(codec.uppercased())
                            .font(.system(size: 9, weight: .semibold).monospaced())
                            .foregroundStyle(.white)
                            .padding(.horizontal, 5)
                            .padding(.vertical, 2)
                            .background(.black.opacity(0.55), in: Capsule())
                            .padding(6)
                    }
                }
                .overlay(alignment: .bottomTrailing) {
                    FavouriteButton(
                        isOn: library.isFavourite(album: album.id),
                        showing: hovering,
                        size: .callout
                    ) {
                        library.toggleFavourite(album: album.id)
                    }
                    // Over artwork, which can be any colour — the plain
                    // tertiary heart disappears against half of them.
                    .shadow(color: .black.opacity(0.6), radius: 3)
                    .padding(7)
                }

            Text(album.title)
                .font(.callout.weight(.medium))
                .underline(titleHovering)
                .lineLimit(1)
                .contentShape(.rect)
                .onHover { titleHovering = $0 }
                .onTapGesture { nav.open(album: album.id) }

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
        .onHover { hovering = $0 }
        .contextMenu { PlayableMenu(playable: .album(album)) }
        .draggablePlayable(.album(album))
    }

}
