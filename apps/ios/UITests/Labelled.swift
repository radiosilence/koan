import XCTest

extension XCUIElementQuery {
    /// The first element labelled `label` in any case, or identified by it.
    /// The kōan theme lowercases the app's own titles on screen, and SwiftUI
    /// lowercases the accessibility label with them, so a test that names a
    /// control as it is written would otherwise miss it.
    subscript(any label: String) -> XCUIElement {
        matching(NSPredicate(format: "label ==[c] %@ OR identifier == %@", label, label)).firstMatch
    }
}
