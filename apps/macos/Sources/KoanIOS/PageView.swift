import SwiftUI

/// Which page the navigator is pointing at, for a tab to push.
///
/// The Mac's `RootView` does the same switch but keeps the browsers mounted
/// behind the stage; a phone's navigation stack already keeps what it pushed,
/// so here each page is simply built.
struct PageView: View {
    @Environment(Navigator.self) private var nav

    @ViewBuilder var body: some View {
        switch nav.current {
        case .section(.queue):
            // Kept alive above, and this is only reached when it is not showing.
            EmptyView()
        case .section(.searchResults):
            SearchResultsView()
        case .section(.albums):
            AlbumBrowser()
        case .section(.artists):
            ArtistBrowser()
        case .section(.favourites):
            FavouritesView()
        case .section(.playHistory):
            HistoryView()
        case .section(.downloads):
            DownloadsView()
        case .section(.playlist(let id)):
            PlaylistView(playlistId: id)
        case .album(let id):
            AlbumDetailView(albumId: id)
        case .artist(let id):
            ArtistDetailView(artistId: id)
        }
    }
}
