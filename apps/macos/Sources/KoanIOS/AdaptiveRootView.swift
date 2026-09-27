import SwiftUI

/// Which shell, decided by how much room there is.
///
/// The split view is not "the desktop layout" and the tab bar is not "the phone
/// layout" — they are the wide one and the narrow one, and the same iPad is
/// both depending on whether it is sharing the screen. Keying on
/// `horizontalSizeClass` rather than on `os()` is what makes that fall out for
/// free instead of needing a third case.
///
/// Platform still decides some things, and should: the menu bar, single-key
/// shortcuts, hover, a settings *window* rather than a settings page. Those are
/// about what the machine is, not about how wide it is. Layout is the part that
/// belongs to the space.
struct AdaptiveRootView: View {
    @Environment(\.horizontalSizeClass) private var width

    var body: some View {
        if width == .compact {
            TabShell()
        } else {
            RootView()
        }
    }
}
