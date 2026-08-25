import SwiftUI

/// Search, as its own tab.
///
/// The Mac keeps the field in the sidebar, where it is always visible. A phone
/// has nowhere to keep it, so it becomes the search tab iOS reserves a slot for
/// — and the results are the same page the Mac shows.
struct IOSSearchView: View {
    @Environment(SearchModel.self) private var search
    @Environment(Navigator.self) private var nav

    var body: some View {
        SearchResultsView()
            .environment(\.onStage, true)
            .navigationTitle("Search")
            .searchable(text: Binding(
                get: { search.query },
                set: { search.query = $0 }
            ))
    }
}
