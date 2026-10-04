import CoreGraphics
import Foundation
import KoanFFI
import Observation
import SwiftUI

/// The bits of presentation a keystroke has to reach.
///
/// Sheets and the search field belong to views, but a hotkey arrives from
/// outside all of them — so the flags live here, where the key monitor can set
/// them and the view that owns the thing can watch. The menu bar drives the
/// same flags, so both routes agree.
@MainActor
@Observable
final class UIState {
    var showingPicker = false
    var showingArtwork = false
    var showingShortcuts = false
    /// An invite that would replace the account already signed in, waiting
    /// for a yes.
    var pendingInvite: Invite?

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
    /// How far in from the trailing edge the lyrics panel starts, for the same
    /// reason — zero while it is closed. Floating over it would cut off the
    /// last lines of a song, which is the part you are usually reading.
    var lyricsWidth: CGFloat = 0

    /// Where the queue can be sent. Two ends and the row the music is on.
    enum Jump { case top, bottom, playing }

    /// Counters, not booleans: pressing `g` twice has to jump twice, and a flag
    /// that is already true is not a change anything can observe.
    private(set) var searchFocusToken = 0
    private(set) var filterFocusToken = 0
    private(set) var queueJumpToken = 0
    private(set) var queueJumpTarget = Jump.top
    /// Escape drops the selection wherever you are, so every list that has one
    /// watches this rather than each binding its own key and disagreeing about
    /// which of them the keystroke belonged to.
    private(set) var clearSelectionToken = 0
    /// ⌘A, likewise: it belongs to whatever list is on screen, and only that
    /// list knows what "everything" is.
    private(set) var selectAllToken = 0

    func focusSearch() { searchFocusToken += 1 }

    func focusFilter() { filterFocusToken += 1 }

    func clearSelection() { clearSelectionToken += 1 }

    func selectAll() { selectAllToken += 1 }

    func jumpQueue(to target: Jump) {
        queueJumpTarget = target
        queueJumpToken += 1
        followingQueue = target == .playing
    }

    /// Whether the queue keeps the playing track in view as it changes.
    /// Turned on by jumping to it, off by any scroll of the person's own.
    /// For the session only, as the queue's scroll position is.
    var followingQueue = false

    /// The jump-to-playing button: jump and follow, or stop following.
    func toggleFollowingQueue() {
        if followingQueue {
            followingQueue = false
        } else {
            jumpQueue(to: .playing)
        }
    }

    /// Whether the lyrics panel is open.
    ///
    /// Observable state that writes through to defaults, rather than
    /// `@AppStorage` on each view that reads it. A `UserDefaults` write
    /// publishes on its own, after the transaction that caused it has gone —
    /// so the inspector would have no animation to expand with and arrive at
    /// full width in a single frame while everything around it was still
    /// sliding. An observed property changes *inside* the transaction, which is
    /// what hands the pane AppKit's own slide.
    ///
    /// One copy, and where you left it across a launch.
    var showLyrics: Bool = UserDefaults.standard.bool(forKey: UIState.lyricsKey) {
        didSet { UserDefaults.standard.set(showLyrics, forKey: UIState.lyricsKey) }
    }

    private static let lyricsKey = "showLyrics"

    /// Explicitly animated: the transaction this opens reaches the inspector's
    /// split view, so the pane slides and the
    /// stage and transport resize with it instead of after it.
    func toggleLyrics() {
        withAnimation(.smooth(duration: 0.28)) { showLyrics.toggle() }
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
