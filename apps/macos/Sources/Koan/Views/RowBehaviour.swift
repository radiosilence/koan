import KoanFFI
import SwiftUI

/// Everything a list row does, in one place.
///
/// Rows across the app need the same behaviours — be selectable, respond to
/// double-click, carry a context menu, and hold their hit area across the full
/// width. Defining that per screen is how they drifted: album
/// and artist lists kept a tap gesture that had already been removed from the
/// queue for breaking selection, so the same bug lived on in two places after
/// being "fixed".
///
/// The hard-won details behind each of these are in `DoubleClick.swift` and
/// `PlayableTransfer.swift`.
struct RowBehaviour: ViewModifier {
    /// What the row stands for. Supplies the drag payload and the menu.
    let playable: Playable?
    /// Run on double-click. Usually "play what is selected".
    let onOpen: () -> Void
    /// Extra items appended to the shared playable menu.
    @ViewBuilder let extraMenu: () -> AnyView

    func body(content: Content) -> some View {
        content
            // Full width and an explicit shape: empty space in a row is not
            // hit-testable, so the gap between title and duration would be dead.
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
            // No drag here. `.onDrag` on a List row claims the press to watch
            // for movement, which leaves single-click selection unreliable —
            // it broke selection in exactly the lists whose rows were
            // draggable, and nowhere else. Grids and pills drag fine because
            // they are not rows competing with a table's own click handling.
            .contextMenu {
                if let playable {
                    PlayableMenu(playable: playable)
                }
                extraMenu()
            }
            // Never a tap gesture: SwiftUI holds the first click back to see if
            // a second follows, which leaves selection feeling dead.
            .onRowDoubleClick(perform: onOpen)
    }
}

extension View {
    /// Standard list-row behaviour: selectable, double-click to open, context
    /// menu, full-width hit area.
    func rowBehaviour(
        playable: Playable?,
        onOpen: @escaping () -> Void
    ) -> some View {
        modifier(
            RowBehaviour(playable: playable, onOpen: onOpen, extraMenu: { AnyView(EmptyView()) })
        )
    }

    /// As above, with extra menu items after the shared ones.
    func rowBehaviour(
        playable: Playable?,
        onOpen: @escaping () -> Void,
        @ViewBuilder extraMenu: @escaping () -> some View
    ) -> some View {
        modifier(
            RowBehaviour(playable: playable, onOpen: onOpen, extraMenu: { AnyView(extraMenu()) })
        )
    }
}
