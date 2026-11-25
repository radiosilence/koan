import Foundation
import KoanFFI
import Observation
import SwiftUI

/// Presentation state that more than one view reaches: what the shell
/// presents, and what a page asks of another.
@MainActor
@Observable
final class UIState {
    /// Settings → Server, asked for by the server banner's Sign In, which the
    /// Settings tab pushes.
    var showingServerSettings = false
    /// Moves when something played from outside the app, as the Top Shelf's
    /// Play does, should bring Now Playing forward.
    var nowPlayingAsked = 0
    /// An invite that would replace the account already signed in, waiting
    /// for a yes.
    var pendingInvite: Invite?
    /// A device waiting to be signed in as this account, waiting for a yes.
    var pendingPairing: PairingRequest?
    /// A device just signed in from here, by its name: if it is an Apple TV,
    /// setting it up to be woken is offered once it links.
    var signedInHere: SignedInHere?
    /// The Apple TV being set up to be woken, while its sheet is up.
    var tvWake: TvWakeTarget?
    /// An Apple TV that has just linked after being signed in from here, for
    /// "set it up to be woken?".
    var tvWakeOffer: TvWakeTarget?

    /// How far in from the trailing edge the lyrics panel starts; zero while
    /// it is closed.
    var lyricsWidth: CGFloat = 0

    /// Where the queue can be sent. Two ends and the row the music is on.
    enum Jump { case top, bottom, playing }

    /// Counters, not booleans: asking twice has to jump twice, and a flag
    /// that is already true is not a change anything can observe.
    private(set) var queueJumpToken = 0
    private(set) var queueJumpTarget = Jump.top
    /// Drops the selection in every list that has one.
    private(set) var clearSelectionToken = 0
    /// Selects everything in the list on screen, which alone knows what
    /// "everything" is.
    private(set) var selectAllToken = 0

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
    /// `@AppStorage` on each view that reads it: one copy, and where you left
    /// it across a launch.
    var showLyrics: Bool = UserDefaults.standard.bool(forKey: UIState.lyricsKey) {
        didSet { UserDefaults.standard.set(showLyrics, forKey: UIState.lyricsKey) }
    }

    private static let lyricsKey = "showLyrics"

    /// Opened and closed in one step, not animated: the page and the panel
    /// are laid out together rather than on two clocks.
    func toggleLyrics() {
        var transaction = Transaction()
        transaction.disablesAnimations = true
        withTransaction(transaction) { showLyrics.toggle() }
    }
}

/// Drops the selection when `clearSelectionToken` moves.
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
