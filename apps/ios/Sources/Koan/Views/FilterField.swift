import SwiftUI

/// Narrows what the current view is showing.
///
/// Deliberately separate from the sidebar's search field. That one is a
/// library-wide lookup that navigates you somewhere; this one only hides rows
/// in front of you, and conflating the two means you cannot say "of the albums
/// I am looking at, the ones with 'live' in the title" without being thrown
/// into a results page.
///
/// The platform's look; the theme's is `KoanSearchField`, chosen by
/// `LibraryFilter`.
///
/// A real `NSSearchField` rather than a `TextField`. SwiftUI offers the search
/// look only through `.searchable`, which the sidebar has already claimed, and
/// every hand-built substitute loses something — the magnifier, the clear
/// button, the focus ring that fits the control instead of haloing it.
///
/// UIKit's `UISearchBar` is a chrome-heavy thing built to sit at the top of a
/// screen, not inline above a list, so iOS gets a plain field instead. The
/// focus token has nothing to drive there either: ⌘F is a keyboard affordance.
struct FilterField: View {
    let placeholder: String
    @Binding var text: String
    var focusToken = 0

    var body: some View {
        TextField(placeholder, text: $text)
            .borderedField()
            .autocorrectionDisabled()
            .textInputAutocapitalization(.never)
    }
}
