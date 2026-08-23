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
        // Only when it actually differs, or every keystroke resets the cursor
        // to the end of the field.
        if field.stringValue != text {
            field.stringValue = text
        }
    }

    func makeCoordinator() -> Coordinator { Coordinator(text: $text) }

    final class Coordinator: NSObject, NSSearchFieldDelegate {
        private let text: Binding<String>

        init(text: Binding<String>) {
            self.text = text
        }

        func controlTextDidChange(_ notification: Notification) {
            guard let field = notification.object as? NSSearchField else { return }
            text.wrappedValue = field.stringValue
        }
    }
}
