import AppKit
import SwiftUI

/// Narrows what the current view is showing.
///
/// Deliberately separate from the sidebar's search field. That one is a
/// library-wide lookup that navigates you somewhere; this one only hides rows
/// in front of you, and conflating the two means you cannot say "of the albums
/// I am looking at, the ones with 'live' in the title" without being thrown
/// into a results page.
///
/// A real `NSSearchField` rather than a `TextField`. SwiftUI offers the search
/// look only through `.searchable`, which the sidebar has already claimed, and
/// every hand-built substitute loses something — the magnifier, the clear
/// button, the focus ring that fits the control instead of haloing it.
struct FilterField: NSViewRepresentable {
    let placeholder: String
    @Binding var text: String
    /// ⌘F. A counter because focusing twice has to work twice.
    var focusToken = 0

    func makeNSView(context: Context) -> NSSearchField {
        let field = NSSearchField()
        field.placeholderString = placeholder
        field.delegate = context.coordinator
        field.sendsSearchStringImmediately = true
        field.sendsWholeSearchString = false
        field.controlSize = .regular
        field.focusRingType = .default
        field.setContentHuggingPriority(.defaultLow, for: .horizontal)
        return field
    }

    func updateNSView(_ field: NSSearchField, context: Context) {
        field.placeholderString = placeholder
        if context.coordinator.claim(focusToken) {
            field.window?.makeFirstResponder(field)
        }
        // Only when it actually differs, or every keystroke resets the cursor
        // to the end of the field.
        if field.stringValue != text {
            field.stringValue = text
        }
    }

    func makeCoordinator() -> Coordinator { Coordinator(text: $text) }

    final class Coordinator: NSObject, NSSearchFieldDelegate {
        private let text: Binding<String>
        private var seenFocusToken = 0

        init(text: Binding<String>) {
            self.text = text
        }

        /// True the first time a given token is seen — the field is rebuilt on
        /// every keystroke in it, and stealing focus back each time would fight
        /// anyone who had clicked elsewhere.
        func claim(_ token: Int) -> Bool {
            guard token != seenFocusToken else { return false }
            seenFocusToken = token
            return true
        }

        func controlTextDidChange(_ notification: Notification) {
            guard let field = notification.object as? NSSearchField else { return }
            text.wrappedValue = field.stringValue
        }
    }
}
