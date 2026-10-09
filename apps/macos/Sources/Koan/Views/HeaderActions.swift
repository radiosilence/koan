import SwiftUI

/// What you can do with the record or artist whose page you are on.
///
/// Laid out in full where there is room: labelled buttons in a row. Where there
/// is not, the words do not simply drop off — a row of bare glyphs says nothing
/// about what it does. They collapse
/// into the overflow menu instead, which is the same `PlayableMenu` a
/// long-press already gives you.
///
/// Favourite stays out of the menu either way. It is a state you want to see
/// rather than an action you go looking for.
struct HeaderActions: View {
    let playable: Playable?
    @Environment(\.horizontalSizeClass) private var width

    var body: some View {
        HStack(spacing: 10) {
            if width == .compact {
                if let playable {
                    FavouriteHeaderButton(playable: playable)
                    Menu {
                        PlayableMenu(playable: playable)
                    } label: {
                        KoanIcon(Icon.more)
                            .font(.role(.body, system: .body))
                            .touchTarget()
                    }
                    .menuStyle(.borderlessButton)
                    .fixedSize()
                }
            } else {
                #if os(tvOS)
                if let playable {
                    PlayableHeaderButton(playable: playable)
                    ShuffleHeaderButton(playable: playable)
                }
                #endif
                QueueButtons(playable: playable)
                if let playable {
                    ShareButton(playable: playable)
                    FavouriteHeaderButton(playable: playable)
                }
            }
        }
        // Compact beside the header's one prominent action, Play; the
        // platform's own buttons otherwise.
        .koanButtons(.compact)
    }
}

extension VerticalAlignment {
    private enum HeaderCentre: AlignmentID {
        static func defaultValue(in d: ViewDimensions) -> CGFloat { d[VerticalAlignment.center] }
    }

    /// The middle of a header's play button (the default), and of its title's first line.
    static let headerCentre = VerticalAlignment(HeaderCentre.self)
}

extension View {
    /// A page's title beside its play button: the middle of the first line's
    /// capitals level with the button's middle, and any further line wrapping
    /// below. The line box's own middle sits below the capitals' middle by the
    /// font's descent, which is what a plain `.center` would get wrong.
    func headerTitleCentre(_ role: KoanType, systemSize: CGFloat) -> some View {
        modifier(HeaderTitleCentre(role: role, systemSize: systemSize))
    }
}

private struct HeaderTitleCentre: ViewModifier {
    let role: KoanType
    let systemSize: CGFloat

    func body(content: Content) -> some View {
        let half = capHeight / 2
        return content.alignmentGuide(.headerCentre) { $0[.firstTextBaseline] - half }
    }

    private var capHeight: CGFloat {
        #if os(macOS)
        NSFont.role(role, system: NSFont.systemFont(ofSize: systemSize, weight: .semibold)).capHeight
        #else
        (KoanTheme.isOn ? UIFont.koan(role) : UIFont.systemFont(ofSize: systemSize, weight: .semibold)).capHeight
        #endif
    }
}
