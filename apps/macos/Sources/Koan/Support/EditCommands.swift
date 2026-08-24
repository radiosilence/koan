import AppKit

/// Edit menu actions, routed by what has focus.
///
/// The queue borrows Cut/Copy/Paste/Select All, but those still have to mean
/// the ordinary thing while you're typing — ⌘A in the search field must select
/// the text, not every track in the queue. So each action asks whether a text
/// view has focus and, if it does, hands the standard selector to the responder
/// chain instead of doing anything itself.
@MainActor
enum EditCommands {
    /// True while a text field or text view is first responder.
    static var isEditingText: Bool {
        guard let responder = NSApp.keyWindow?.firstResponder else { return false }
        // A focused NSTextField is represented by its field editor, an
        // NSTextView whose delegate is the field — so checking for NSTextView
        // covers both.
        return responder is NSTextView || responder is NSTextField
    }

    /// Runs `action` unless text is being edited, in which case the standard
    /// editing selector is sent down the responder chain.
    static func route(_ selector: Selector, otherwise action: () -> Void) {
        if isEditingText {
            NSApp.sendAction(selector, to: nil, from: nil)
        } else {
            action()
        }
    }

    static func selectAll(otherwise action: () -> Void) {
        route(#selector(NSText.selectAll(_:)), otherwise: action)
    }

    static func copy(otherwise action: () -> Void) {
        route(#selector(NSText.copy(_:)), otherwise: action)
    }

    static func cut(otherwise action: () -> Void) {
        route(#selector(NSText.cut(_:)), otherwise: action)
    }

    static func paste(otherwise action: () -> Void) {
        route(#selector(NSText.paste(_:)), otherwise: action)
    }

    static func delete(otherwise action: () -> Void) {
        route(#selector(NSText.delete(_:)), otherwise: action)
    }
}
