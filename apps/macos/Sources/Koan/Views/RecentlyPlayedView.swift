import KoanFFI
import SwiftUI

/// What you played lately: the records, artists and tracks of the last month,
/// newest first, each once however often it played. Answers "what was that
/// album I had on yesterday", which History, every play by day, does not.
/// Derived from the play history, so it follows each play as it is recorded.
struct RecentlyPlayedView: View {
    @Environment(LibraryModel.self) private var library

    var body: some View {
        ShelfView(
            title: "Recently Played",
            shelf: .recent,
            summary: library.visibleShelf,
            empty: EmptyShelf(
                icon: Icon.recentlyPlayed,
                title: "Nothing played lately",
                detail: "What you play shows up here, newest first."
            )
        )
    }
}
