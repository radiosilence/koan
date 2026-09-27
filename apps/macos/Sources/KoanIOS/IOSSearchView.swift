import SwiftUI

/// Search, as its own tab.
///
/// The Mac keeps the field in the sidebar, where it is always visible. A phone
/// has nowhere to keep it, so it becomes the search tab iOS reserves a slot for
/// — and the results are the same page the Mac shows.
struct IOSSearchView: View {
    @Environment(SearchModel.self) private var search

    var body: some View {
        SearchResultsView()
            .environment(\.onStage, true)
            // Always showing, under the title. Left to itself the field hides
            // until the page is pulled down, and on the one page whose whole
            // purpose is the field, that reads as there being none.
            .searchable(
                text: Binding(get: { search.query }, set: { search.query = $0 }),
                placement: .navigationBarDrawer(displayMode: .always),
                prompt: "Artists, albums, tracks"
            )
            .onSubmit(of: .search) { search.submit() }
    }
}
