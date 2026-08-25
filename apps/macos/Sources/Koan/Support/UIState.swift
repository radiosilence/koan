import CoreGraphics
import Foundation
import Observation
import SwiftUI

/// The bits of presentation a keystroke has to reach.
///
/// Sheets, the search field and the queue's scroll position belong to views,
/// but a hotkey arrives from outside all of them — so the flags live here,
/// where the key monitor can set them and the view that owns the thing can
/// watch. The menu bar drives the same flags, so both routes agree.
@MainActor
@Observable
final class UIState {
    var showingPicker = false
    var showingArtwork = false
    var showingShortcuts = false

    /// The main window's content size, measured by `RootView`.
    ///
    /// A sheet is bounded by the window it hangs from but cannot ask how big
    /// that is — a `GeometryReader` inside one only ever sees the sheet's own
    /// proposal. The window measures itself and leaves the answer here.
    var windowSize: CGSize = .zero
    /// How far in the stage starts. The transport floats over the window — the
    /// only place a `NavigationStack` push cannot drop it — so it needs to know
    /// where the sidebar ends in order not to sit on it.
    var sidebarWidth: CGFloat = 0

    enum Edge { case top, bottom }

    /// Counters, not booleans: pressing `g` twice has to jump twice, and a flag
    /// that is already true is not a change anything can observe.
    private(set) var searchFocusToken = 0
    private(set) var filterFocusToken = 0
    private(set) var queueJumpToken = 0
    private(set) var queueJumpEdge = Edge.top
    /// Escape drops the selection wherever you are, so every list that has one
    /// watches this rather than each binding its own key and disagreeing about
    /// which of them the keystroke belonged to.
    private(set) var clearSelectionToken = 0
    /// ⌘A, likewise: it belongs to whatever list is on screen, and only that
    /// list knows what "everything" is. It lived on the player, so it only ever
    /// reached the queue.
    private(set) var selectAllToken = 0

    /// Where the queue was left, so coming back to it is coming back rather
    /// than starting again. The page is a `switch` in one view, so leaving the
    /// queue destroys it and takes its scroll position with it.
    var queueScrollY: CGFloat = 0

    func focusSearch() { searchFocusToken += 1 }

    func focusFilter() { filterFocusToken += 1 }

    func clearSelection() { clearSelectionToken += 1 }

    func selectAll() { selectAllToken += 1 }

    func jumpQueue(to edge: Edge) {
        queueJumpEdge = edge
        queueJumpToken += 1
    }

    /// The panel's state is `@AppStorage`, so it survives a launch and there is
    /// no second copy of it here to disagree with.
    func toggleLyrics() {
        let defaults = UserDefaults.standard
        defaults.set(!defaults.bool(forKey: "showLyrics"), forKey: "showLyrics")
    }
}

/// Escape drops the selection, wherever you are.
///
/// The key is caught once, by the monitor, because a `List` that has focus is
/// the only thing that would ever see it otherwise — and only one list has
/// focus. Every list carries this instead and clears itself when the token
/// moves.
private struct ClearsSelection<Value: Hashable>: ViewModifier {
    @Environment(UIState.self) private var ui
    @Binding var selection: Set<Value>

    func body(content: Content) -> some View {
        content.onChange(of: ui.clearSelectionToken) { _, _ in selection = [] }
    }
}

extension View {
    func clearsSelection<Value: Hashable>(_ selection: Binding<Set<Value>>) -> some View {
        modifier(ClearsSelection(selection: selection))
    }
}
