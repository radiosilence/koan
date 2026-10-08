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
        #if os(tvOS)
        television
        #else
        tile
        #endif
    }

    #if os(tvOS)
    /// The tile as one card a remote focuses and clicks to open the record.
    /// Playing it is a click away on the record's page, or in the menu a long
    /// press brings up.
    private var television: some View {
        VStack(alignment: .leading, spacing: 14) {
            Button { nav.open(album: album.id) } label: {
                AlbumArtwork(source: .album(album.id), size: .tile, cornerRadius: KoanTheme.radius(10))
            }
            .buttonStyle(.card)
            .contextMenu { PlayableMenu(playable: .album(album)) }

            VStack(alignment: .leading, spacing: 2) {
                Text(album.title)
                    .font(.role(.control, system: .callout.weight(.medium)))
                    .lineLimit(1)
                Text([showArtist ? album.artistName : nil, album.year.map { String($0) }]
                    .compactMap { $0 }
                    .joined(separator: " · "))
                    .font(.role(.fine, system: .caption))
                    .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                    .lineLimit(1)
            }
        }
    }

    #endif

    private var tile: some View {
        VStack(alignment: .leading, spacing: 7) {
            PlayableArtwork(albumId: album.id)
                .koanShadow(0.28, radius: 7, y: 3)
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
                            .font(.role(.fine, system: .system(size: 9, weight: .semibold).monospaced()))
                            .foregroundStyle(KoanTheme.style(.ink, system: .white))
                            .padding(.horizontal, 6)
                            .padding(.vertical, 3)
                            // Clear glass, not a black scrim: over artwork the
                            // point is to stay readable without hiding the
                            // corner of the cover it sits on.
                            .glass(.clear, fallback: .ultraThinMaterial, in: .capsule)
                            .padding(6)
                    }
                }
                #if os(iOS)
                .overlay {
                    if album.onDevice != nil {
                        DownloadedBar(album: album)
                    }
                }
                #endif
                .overlay(alignment: .bottomTrailing) {
                    AlbumTileHeart(albumId: album.id, hovering: hovering)
                }

            Text(album.title)
                .font(.role(.control, system: .callout.weight(.medium)))
                .underline(titleHovering)
                .lineLimit(1)
                .contentShape(.rect)
                .pointerHover { titleHovering = $0 }
                .onTapGesture { Trace.event("tap"); nav.open(album: album.id) }

            HStack(spacing: 4) {
                if showArtist {
                    LinkText(text: album.artistName, target: .artist(album.artistId), font: .role(.fine, system: .caption))
                }
                if let year = album.year {
                    Text(showArtist ? "· \(String(year))" : String(year))
                        .font(.role(.fine, system: .caption))
                        .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                }
            }
        }
        .pointerHover { hovering = $0 }
        .animation(.smooth(duration: 0.18), value: hovering)
        // While selecting, the whole tile is one target that ticks it — the art
        // does not play and the links do not go anywhere.
        .accessibilityHidden(selecting)
        .overlay {
            if selecting, let selection {
                SelectionTarget(playable: .album(album), selection: selection)
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
            // Padded inside the button, so the disc drawn behind it is what
            // takes the click rather than the tile beneath.
            AlbumHeart(albumId: albumId, size: .callout, inset: 7)
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
        RoundedRectangle(cornerRadius: KoanTheme.radius(6))
            .strokeBorder(selected ? AnyShapeStyle(.tint) : AnyShapeStyle(.clear), lineWidth: 3)
            .overlay(alignment: .topLeading) {
                KoanIcon(selected ? Icon.picked : Icon.unpicked, palette: true)
                    .font(.system(size: 20))
                    .symbolRenderingMode(.palette)
                    .foregroundStyle(.white, selected ? AnyShapeStyle(.tint) : AnyShapeStyle(.black.opacity(0.25)))
                    .koanShadow(0.35, radius: 2)
                    .padding(7)
            }
    }
}

/// A tile while selecting: one target that ticks it, which VoiceOver reads as
/// the record and whether it is picked. Its own view, as the mark is.
private struct SelectionTarget: View {
    let playable: Playable
    let selection: PlayableSelection

    var body: some View {
        Color.clear
            .contentShape(.rect)
            .onTapGesture { selection.click(playable) }
            .accessibilityElement()
            .accessibilityLabel(playable.name)
            .accessibilityAddTraits(selection.contains(playable.key) ? [.isButton, .isSelected] : .isButton)
            .accessibilityAction { selection.click(playable) }
    }
}

/// A tick for a row or a pill, where there is no artwork to ring. Its own view
/// so that a tick re-runs it and not the row.
struct SelectionTick: View {
    let key: Playable.Key
    let selection: PlayableSelection

    var body: some View {
        let selected = selection.contains(key)
        KoanIcon(selected ? Icon.picked : Icon.unpicked)
            .foregroundStyle(selected ? AnyShapeStyle(.tint) : KoanTheme.style(.muted, system: .tertiary))
            .accessibilityLabel(selected ? "Selected" : "Not selected")
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
