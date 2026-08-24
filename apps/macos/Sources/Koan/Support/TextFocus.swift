import AppKit
import Observation

/// Whether someone is typing, observably.
///
/// This exists so menu commands can be *disabled* while a field has focus,
/// which is the only thing that actually hands the key back to macOS. A
/// disabled menu item does not claim its key equivalent, so the event carries
/// on down the responder chain and the field does what every other app would
/// do with it — ⌥← moves a word, ⌘← goes to the start of the line, ⌘Z undoes
/// the typing.
///
/// Declining the *action* is not enough and is worse than doing nothing: the
/// menu still swallows the key, so the shortcut stops working and the field
/// never hears about it either.
///
/// Tracked from the field editor's own notifications rather than by polling the
/// first responder, because `.commands` is part of the Scene body and has to be
/// invalidated when this changes.
@MainActor
@Observable
final class TextFocus {
    private(set) var isEditing = false

    @ObservationIgnored private var observers: [NSObjectProtocol] = []

    init() {
        let centre = NotificationCenter.default
        // Posted by the field editor, so this covers every text control that
        // uses one — TextField, TextEditor, and the search field.
        observers.append(
            centre.addObserver(
                forName: NSText.didBeginEditingNotification, object: nil, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated { self?.isEditing = true }
            }
        )
        observers.append(
            centre.addObserver(
                forName: NSText.didEndEditingNotification, object: nil, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated { self?.isEditing = false }
            }
        )
        // A window losing key while a field is focused ends editing without
        // ending it — leaving every shortcut disabled until someone clicks back
        // into the field and out again.
        observers.append(
            centre.addObserver(
                forName: NSWindow.didResignKeyNotification, object: nil, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated { self?.refresh() }
            }
        )
        observers.append(
            centre.addObserver(
                forName: NSWindow.didBecomeKeyNotification, object: nil, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated { self?.refresh() }
            }
        )
    }

    /// Ask the responder chain directly. The notifications say when editing
    /// starts and stops; this is for the moments they cannot describe, like
    /// switching to a window that already had a focused field.
    private func refresh() {
        isEditing = EditCommands.isEditingText
    }

    /// Nothing unregisters these. This lives as long as the app does, and a
    /// nonisolated deinit cannot reach main-actor state to do it anyway.
}
