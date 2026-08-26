import SwiftUI

/// Lets a tab show a page the navigator moved to.
///
/// koan navigates like a browser: opening a record moves the navigator to a
/// page that is not a section, and a tab that only ever draws its own content —
/// the queue, the search results — has nowhere to put it. The action then looks
/// like it did nothing, which is exactly how "Go to Album" behaved from a queue
/// row and from a search result.
///
/// Pushing it onto the tab's own stack also keeps Back honest: it returns you
/// to the queue, or to the results, rather than to some other tab.
private struct DetailPush: ViewModifier {
    @Environment(Navigator.self) private var nav

    func body(content: Content) -> some View {
        content.navigationDestination(isPresented: Binding(
            get: { nav.current.section == nil },
            set: { presented in if !presented { nav.goBack() } }
        )) {
            PageView()
                .environment(\.onStage, true)
        }
    }
}

extension View {
    func pushesDetailPages() -> some View { modifier(DetailPush()) }
}
