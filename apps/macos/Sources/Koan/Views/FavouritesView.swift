import KoanFFI
import SwiftUI

/// Everything you have favourited, in one page: artists, records and tracks.
struct FavouritesView: View {
    @Environment(LibraryModel.self) private var library

    var body: some View {
        ShelfView(
            title: "Favourites",
            shelf: .favourites,
            summary: library.visibleShelf,
            empty: EmptyShelf(
                icon: "heart",
                title: "Nothing favourited yet",
                detail: "Hit the heart on a track, a record or an artist."
            )
        )
    }
}
