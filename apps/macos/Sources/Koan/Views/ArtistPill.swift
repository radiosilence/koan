import KoanFFI
import SwiftUI

/// An artist as a tappable chip, wherever artists are listed among other
/// results.
struct ArtistPill: View {
    let name: String
    let artistId: Int64
    /// The page's pick, where the pill takes part in one — see
    /// `PlayableSelection`.
    var selection: PlayableSelection?

    @Environment(Navigator.self) private var nav

    var body: some View {
        HStack(spacing: 5) {
            Group {
                if let selection, selection.isActive {
                    SelectionTick(key: playable.key, selection: selection)
                } else {
                    KoanIcon(Icon.artist)
                        .foregroundStyle(KoanTheme.style(.muted, system: .tertiary))
                }
            }
            .font(.role(.fine, system: .caption2))
            // A classical release credits the soloist, the orchestra and the
            // conductor in one artist string, which as a pill is a paragraph
            // laid on its side. The full name is in the tooltip and on the
            // artist page.
            Text(name)
                .font(.role(.control, system: .callout))
                .lineLimit(1)
                .truncationMode(.tail)
        }
        .frame(maxWidth: 260, alignment: .leading)
        .padding(.horizontal, 11)
        .padding(.vertical, 6)
        .fixedSize(horizontal: true, vertical: false)
        // A plain fill rather than glass. Glass samples what is behind it and
        // adapts its own luminance to stay legible on it — which is right for
        // something floating over content, and wrong for a chip sitting *in*
        // it: over the wash a row of them reads as a scatter of
        // half-transparent ones rather than a set. Square on `rule` in the
        // theme, the Mac collection's ground for its pills; a capsule otherwise.
        .background(KoanTheme.style(.rule), in: Self.shape)
        .contentShape(Self.shape)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.isButton)
        .tapAction {
            if selection?.take(playable) == true { return }
            nav.open(artist: artistId)
        }
        .help("Go to \(name)")
        .contextMenu { PlayableMenu(playable: playable) }
        .modifier(SelectableDrag(playable: playable, inContainer: selection != nil))
    }

    private var playable: Playable { .artist(id: artistId, name: name) }

    /// A capsule, square in the theme: SwiftUI holds a corner to half the
    /// shorter side, and `.circular` is the corner `Capsule` draws.
    private static var shape: RoundedRectangle {
        RoundedRectangle(cornerRadius: KoanTheme.radius(100), style: .circular)
    }
}
