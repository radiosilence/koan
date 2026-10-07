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
    var shuffle: (() -> Void)?
    @Environment(\.horizontalSizeClass) private var width

    var body: some View {
        HStack(spacing: 10) {
            if width == .compact {
                if let playable {
                    FavouriteHeaderButton(playable: playable)
                    Menu {
                        if let shuffle {
                            Button(action: shuffle) {
                                Label("Shuffle", systemImage: Icon.shuffle)
                            }
                        }
                        PlayableMenu(playable: playable)
                    } label: {
                        Image(systemName: "ellipsis")
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
                }
                #endif
                QueueButtons(playable: playable)
                if let shuffle {
                    Button(action: shuffle) {
                        Label("Shuffle", systemImage: Icon.shuffle)
                    }
                }
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

extension View {
    /// A page's title beside its play button: the first line's capitals level
    /// with the button's top edge, and any further line wrapping below. Used
    /// in an `HStack(alignment: .top)`, whose plain `.top` would put the line
    /// box there, a font's ascent above the capitals.
    func headerTitleTop(_ role: KoanType, systemSize: CGFloat) -> some View {
        modifier(HeaderTitleTop(role: role, systemSize: systemSize))
    }
}

private struct HeaderTitleTop: ViewModifier {
    let role: KoanType
    let systemSize: CGFloat

    func body(content: Content) -> some View {
        let cap = capHeight
        return content.alignmentGuide(.top) { $0[.firstTextBaseline] - cap }
    }

    private var capHeight: CGFloat {
        #if os(macOS)
        NSFont.role(role, system: NSFont.systemFont(ofSize: systemSize, weight: .semibold)).capHeight
        #else
        (KoanTheme.isOn ? UIFont.koan(role) : UIFont.systemFont(ofSize: systemSize, weight: .semibold)).capHeight
        #endif
    }
}
