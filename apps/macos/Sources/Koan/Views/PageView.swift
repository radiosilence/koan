import SwiftUI

/// Which page the navigator is pointing at.
///
/// The shell around it differs per platform — a split view on the Mac, a stack
/// under a tab bar on a phone — but what a section means does not.
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
        case .section(.playlist(let id)):
            PlaylistView(playlistId: id)
        case .album(let id):
            AlbumDetailView(albumId: id)
        case .artist(let id):
            ArtistDetailView(artistId: id)
        }
    }
}
