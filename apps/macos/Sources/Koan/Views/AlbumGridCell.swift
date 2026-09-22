import KoanFFI
import SwiftUI

/// One album in a grid. Shared by the library browser and the artist page so
/// they behave identically — art plays the record, the title opens it, the
/// artist name links out.
struct AlbumGridCell: View {
    let album: Album
    /// An artist's own page already says whose records these are.
    var showArtist: Bool = true
    /// Takes part in picking several records at once — see `AlbumSelection`.
    var selectable = false

    @Environment(PlayerModel.self) private var player
    @Environment(Navigator.self) private var nav
    @Environment(LibraryModel.self) private var library

    @State private var titleHovering = false
    @State private var hovering = false

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            PlayableArtwork(albumId: album.id)
                .shadow(color: .black.opacity(0.28), radius: 7, y: 3)
                .overlay {
                    if selecting { SelectionMark(albumId: album.id) }
                }
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
        // While selecting, the whole tile is one target that ticks it — the art
        // does not play and the links do not go anywhere.
        .overlay {
            if selecting {
                Color.clear
                    .contentShape(.rect)
                    .onTapGesture {
                        library.selection.click(album.id, in: library.visibleAlbums)
                    }
            }
        }
        // ⌘-click starts a selection with this one in it. Over the art's own
        // tap, which would otherwise play the record as well.
        .highPriorityGesture(
            TapGesture().modifiers(.command).onEnded {
                library.selection.begin(with: album.id)
            },
            including: selectable && !selecting ? .all : .subviews
        )
        .contextMenu { PlayableMenu(playable: .album(album)) }
        .modifier(AlbumDrag(album: album, inContainer: selectable))
    }

    /// Read here and nowhere else in the tile: it flips entering and leaving
    /// the mode, not on every tick.
    private var selecting: Bool { selectable && library.selection.isActive }

}

/// The tick on a tile, and the only part of it that reads what is selected — a
/// tick re-runs these and nothing else in the grid.
private struct SelectionMark: View {
    let albumId: Int64
    @Environment(LibraryModel.self) private var library

    var body: some View {
        let selected = library.selection.contains(albumId)
        RoundedRectangle(cornerRadius: 6)
            .strokeBorder(selected ? AnyShapeStyle(.tint) : AnyShapeStyle(.clear), lineWidth: 3)
            .overlay(alignment: .topLeading) {
                Image(systemName: selected ? "checkmark.circle.fill" : "circle")
                    .font(.system(size: 20))
                    .symbolRenderingMode(.palette)
                    .foregroundStyle(.white, selected ? AnyShapeStyle(.tint) : AnyShapeStyle(.black.opacity(0.25)))
                    .shadow(color: .black.opacity(0.35), radius: 2)
                    .padding(7)
            }
    }
}

/// A tile in a selectable grid is an item of the grid's drag container, which
/// says what a drag carries — the whole selection when the tile is part of it.
/// Anywhere else it drags itself.
private struct AlbumDrag: ViewModifier {
    let album: Album
    let inContainer: Bool

    func body(content: Content) -> some View {
        if inContainer {
            content.draggable(containerItemID: album.id)
        } else {
            content.draggablePlayable(.album(album))
        }
    }
}
