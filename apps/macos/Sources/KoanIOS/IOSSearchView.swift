import SwiftUI

/// Search, as its own tab.
///
/// The Mac keeps the field in the sidebar, where it is always visible. A phone
/// has nowhere to keep it, so it becomes the search tab iOS reserves a slot for
/// — and the results are the same page the Mac shows.
struct IOSSearchView: View {
    @Environment(SearchModel.self) private var search

    var body: some View {
        @Bindable var search = search
        SearchResultsView()
            .environment(\.onStage, true)
            // Always showing, under the title. Left to itself the field hides
            // until the page is pulled down, and on the one page whose whole
            // purpose is the field, that reads as there being none.
            .searchable(
                text: $search.query,
                placement: Self.placement,
                prompt: KoanTheme.label("Artists, albums, tracks")
            )
            .onSubmit(of: .search) { search.submit() }
    }

    #if os(tvOS)
    private static let placement = SearchFieldPlacement.automatic
    #else
    private static let placement = SearchFieldPlacement.navigationBarDrawer(displayMode: .always)
    #endif
}
