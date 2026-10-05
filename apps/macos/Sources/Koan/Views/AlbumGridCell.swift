import KoanFFI
import SwiftUI

/// One album in a grid. Shared by the library browser and the artist page so
/// they behave identically — art plays the record, the title opens it, the
/// artist name links out.
struct AlbumGridCell: View {
    let album: Album
    /// An artist's own page already says whose records these are.
    var showArtist = true
    /// The grid's pick, when it takes part in picking several records at once
    /// — see `PlayableSelection`.
    var selection: PlayableSelection?

    @Environment(Navigator.self) private var nav

    @State private var titleHovering = false
    @State private var hovering = false

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            PlayableArtwork(albumId: album.id)
                .shadow(color: .black.opacity(0.28), radius: 7, y: 3)
                .overlay {
                    if selecting, let selection {
                        SelectionMark(key: Playable.album(album).key, selection: selection)
                    }
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
                .overlay {
                    if let fraction = album.downloaded {
                        DownloadedBar(fraction: fraction)
                    }
                }
                .overlay(alignment: .bottomTrailing) {
                    AlbumTileHeart(albumId: album.id, hovering: hovering)
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
            if selecting, let selection {
                Color.clear
                    .contentShape(.rect)
                    .onTapGesture { selection.click(.album(album)) }
            }
        }
        // ⌘-click starts a selection with this one in it. Over the art's own
        // tap, which would otherwise play the record as well.
        #if os(macOS)
        .highPriorityGesture(
            TapGesture().modifiers(.command).onEnded {
                selection?.begin(with: .album(album))
            },
            including: selection != nil && !selecting ? .all : .subviews
        )
        #endif
        .contextMenu { PlayableMenu(playable: .album(album)) }
        .modifier(SelectableDrag(playable: .album(album), inContainer: selection != nil))
    }

    /// Read here and nowhere else in the tile: it flips entering and leaving
    /// the mode, not on every tick.
    private var selecting: Bool { selection?.isActive ?? false }

}

/// The heart on a tile, and the only part of it that reads the favourites — a
/// heart flipping anywhere re-runs these and not the tiles.
///
/// Over artwork, which can be any colour — the plain tertiary heart disappears
/// against half of them. Glass gives it a ground of its own, so the shape is
/// legible whatever is behind it, and it grows in on hover rather than fading
/// a shadowed glyph up.
private struct AlbumTileHeart: View {
    let albumId: Int64
    let hovering: Bool

    @Environment(LibraryModel.self) private var library

    var body: some View {
        if hovering || library.isFavourite(album: albumId) {
            AlbumHeart(albumId: albumId, size: .callout)
                .padding(7)
                .glass(.clear.interactive(), fallback: .ultraThinMaterial, in: .circle)
                .glassEffectTransition(.materialize)
                .padding(7)
        }
    }
}

/// The tick on a tile, and the only part of it that reads what is selected — a
/// tick re-runs these and nothing else in the grid.
private struct SelectionMark: View {
    let key: Playable.Key
    let selection: PlayableSelection

    var body: some View {
        let selected = selection.contains(key)
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

/// A tick for a row or a pill, where there is no artwork to ring. Its own view
/// so that a tick re-runs it and not the row.
struct SelectionTick: View {
    let key: Playable.Key
    let selection: PlayableSelection

    var body: some View {
        let selected = selection.contains(key)
        Image(systemName: selected ? "checkmark.circle.fill" : "circle")
            .foregroundStyle(selected ? AnyShapeStyle(.tint) : AnyShapeStyle(.tertiary))
    }
}

/// An item on a selectable page is an item of the page's drag container, which
/// says what a drag carries — the whole selection when the item is part of it.
/// Anywhere else it drags itself.
struct SelectableDrag: ViewModifier {
    let playable: Playable
    let inContainer: Bool

    func body(content: Content) -> some View {
        // A drag container is the Mac's: the iOS SDKs koan builds against mark
        // it unavailable or newer than the target. On a phone an item drags
        // itself, which is all a touch drag ever carries anyway.
        #if os(macOS)
        if inContainer {
            content.draggable(containerItemID: playable.key)
        } else {
            content.draggablePlayable(playable)
        }
        #else
        content.draggablePlayable(playable)
        #endif
    }
}

/// How much of a record is on this device, along the foot of its sleeve.
private struct DownloadedBar: View {
    let fraction: Double

    var body: some View {
        GeometryReader { geo in
            let side = geo.size.width
            Capsule()
                .fill(.black.opacity(0.35))
                .overlay(alignment: .leading) {
                    Capsule()
                        .fill(.tint)
                        .frame(width: (side - 16 - 34) * min(max(fraction, 0), 1))
                }
                .frame(width: max(side - 16 - 34, 0), height: 3)
                .offset(x: 8, y: side - 8 - 3)
        }
        .aspectRatio(1, contentMode: .fit)
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}
