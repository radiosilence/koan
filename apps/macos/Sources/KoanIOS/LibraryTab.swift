import SwiftUI

/// Everything you browse, behind one tab.
///
/// The Mac lists albums, artists, favourites, playlists and history side by side
/// in the sidebar, where they cost nothing. As tabs they cost a great deal: past
/// five, iOS folds the rest into More — and More is itself a navigation
/// controller, so a tab that brings its own `NavigationStack` arrives with two
/// back buttons stacked on top of each other.
///
/// One Library tab holding all five is both the fix and the better shape. It is
/// what Music does, and it leaves the tab bar saying what koan is actually for:
/// the queue, the library, finding something, and settings.
struct LibraryTab: View {
    @Environment(Navigator.self) private var nav

    var body: some View {
        List {
            row("Albums", Icon.album, .albums)
            row("Artists", Icon.artist, .artists)
            row("Favourites", Icon.favourite, .favourites)
            NavigationLink {
                PlaylistsList()
            } label: {
                Label("Playlists", systemImage: Icon.playlist)
            }
            row("History", Icon.history, .playHistory)
        }
        .navigationTitle("Library")
    }

    /// A section of the library, as a row that goes into it.
    ///
    /// The navigator is moved on the way in rather than by the page itself:
    /// pages read where they are, they do not decide it.
    private func row(
        _ title: String,
        _ symbol: String,
        _ section: Navigator.Section
    ) -> some View {
        NavigationLink {
            PageView()
                .environment(\.onStage, true)
                .navigationTitle(title)
                .onAppear { nav.show(section) }
        } label: {
            Label(title, systemImage: symbol)
        }
    }
}
