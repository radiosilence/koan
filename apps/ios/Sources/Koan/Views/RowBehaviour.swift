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
    #if os(iOS)
    @Environment(\.horizontalSizeClass) private var width
    #endif

    func body(content: Content) -> some View {
        content
            // Empty space in a row is not hit-testable, so the gap between the
            // title and the duration would otherwise be dead.
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
            .modifier(OptionalDrag(playable: playable))
            .washedRow()
            #if os(iOS)
            .listRowInsets(width == .compact ? RowMetrics.compactInsets : nil)
            #endif
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
            content.draggablePlayable(playable)
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
