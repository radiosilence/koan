import AppKit
import KoanFFI
import SwiftUI

/// What a list row itself is responsible for.
///
/// Deliberately small. Selection, the context menu and double-click all belong
/// to the List via `contextMenu(forSelectionType:menu:primaryAction:)`, which is
/// wired into its selection machinery rather than the gesture system — the
/// reason it doesn't steal the first click the way `.onTapGesture(count: 2)`
/// does. Rows only need a hit area and, where it applies, a drag payload.
struct RowBehaviour: ViewModifier {
    let playable: Playable?

    func body(content: Content) -> some View {
        content
            // Empty space in a row is not hit-testable, so the gap between the
            // title and the duration would otherwise be dead.
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
            .modifier(OptionalDrag(playable: playable))
    }
}

/// `.draggable` — not `.onDrag`. The underlying AppKit drag recogniser has a
/// movement threshold, so it coexists with selection; `.onDrag` claims the
/// press outright and leaves clicks landing about one in twenty.
///
/// Known limit: with a multi-selection this drags only the row you grabbed, not
/// the selection. Dragging a whole selection means building the item providers
/// from `selection` by hand.
private struct OptionalDrag: ViewModifier {
    let playable: Playable?

    func body(content: Content) -> some View {
        if let playable {
            content.draggable(PlayableTransfer(playable))
        } else {
            content
        }
    }
}

extension View {
    /// Standard row: full-width hit area, and draggable when it stands for
    /// something playable.
    func rowBehaviour(playable: Playable? = nil) -> some View {
        modifier(RowBehaviour(playable: playable))
    }
}

/// A link inside a row.
///
/// A `Button` wins hit-testing within its own frame, so the row's selection and
/// primary action don't fire when you hit the link — which is what makes
/// "click the artist name to go to the artist, click the row to select it"
/// possible at all. It has to be a Button for that reason; it just shouldn't
/// look like one.
///
/// Styled to match the links elsewhere in the app: ordinary text that underlines
/// on hover. Link blue in a list of names reads as decoration rather than
/// emphasis.
struct RowLink: View {
    let title: String
    let font: Font
    let action: () -> Void

    @State private var hovering = false

    init(_ title: String, font: Font = .body, action: @escaping () -> Void) {
        self.title = title
        self.font = font
        self.action = action
    }

    var body: some View {
        Button(action: action) {
            Text(title)
                .font(font)
                .underline(hovering)
                .foregroundStyle(.primary)
                .lineLimit(1)
        }
        .buttonStyle(.plain)
        .onHover { inside in
            hovering = inside
            // `.pointerStyle(.link)` needs macOS 15; the app targets 14.
            if inside { NSCursor.pointingHand.push() } else { NSCursor.pop() }
        }
    }
}
