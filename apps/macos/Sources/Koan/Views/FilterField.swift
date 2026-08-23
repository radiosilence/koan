import SwiftUI

/// Narrows what the current view is showing.
///
/// Deliberately separate from the sidebar's search field. That one is a
/// library-wide lookup that navigates you somewhere; this one only hides rows
/// in front of you, and conflating the two means you cannot say "of the albums
/// I am looking at, the ones with 'live' in the title" without being thrown
/// into a results page.
///
/// Plain `.roundedBorder` rather than a hand-drawn capsule: it sits next to
/// system controls in the toolbar and anything bespoke reads as a mistake
/// beside them.
struct FilterField: View {
    let placeholder: String

    @Environment(LibraryModel.self) private var library
    @FocusState private var focused: Bool

    var body: some View {
        @Bindable var library = library

        TextField(placeholder, text: $library.filter)
            .textFieldStyle(.roundedBorder)
            .focused($focused)
            // Escape is what people press to abandon a filter.
            .onExitCommand { library.filter = "" }
            .frame(width: 170)
            .onReceive(NotificationCenter.default.publisher(for: .koanFocusFilter)) { _ in
                focused = true
            }
    }
}

extension Notification.Name {
    /// ⌘F, from the Edit menu — the field is in the toolbar and the menu item
    /// has no way to reach into it.
    static let koanFocusFilter = Notification.Name("koan.focusFilter")
}
