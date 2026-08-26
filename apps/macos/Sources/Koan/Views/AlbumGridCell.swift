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
                            .padding(.horizontal, 6)
                            .padding(.vertical, 3)
                            // Clear glass, not a black scrim: over artwork the
                            // point is to stay readable without hiding the
                            // corner of the cover it sits on.
                            .glass(.clear, fallback: .ultraThinMaterial, in: .capsule)
                            .padding(6)
                    }
                }
                .overlay(alignment: .bottomTrailing) {
                    // Over artwork, which can be any colour — the plain
                    // tertiary heart disappears against half of them. Glass
                    // gives it a ground of its own, so the shape is legible
                    // whatever is behind it, and it grows in on hover rather
                    // than fading a shadowed glyph up.
                    if hovering || library.isFavourite(album: album.id) {
                        FavouriteButton(
                            isOn: library.isFavourite(album: album.id),
                            size: .callout
                        ) {
                            library.toggleFavourite(album: album.id)
                        }
                        .padding(7)
                        .glass(.clear.interactive(), fallback: .ultraThinMaterial, in: .circle)
                        .glassEffectTransition(.materialize)
                        .padding(7)
                    }
                }

            Text(album.title)
                .font(.callout.weight(.medium))
                .underline(titleHovering)
                .lineLimit(1)
                .contentShape(.rect)
                .onHover { titleHovering = $0 }
                .onTapGesture { Trace.event("tap"); nav.open(album: album.id) }

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
        .animation(.smooth(duration: 0.18), value: hovering)
        .contextMenu { PlayableMenu(playable: .album(album)) }
        .draggablePlayable(.album(album))
    }

}
