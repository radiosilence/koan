import KoanFFI
import SwiftUI

/// The playlists, as a page rather than a sidebar section.
///
/// The Mac lists them in its sidebar because it has the room. Here they are a
/// page in the library, and each leads to the same `PlaylistView` the Mac shows.
struct PlaylistsList: View {
    private static let emptyDetail = "Playlists made on your phone, your computer or your server show up here."

    @Environment(PlaylistsModel.self) private var playlists
    /// A remote has no other way through a long list than one row at a time.
    @State private var filter = ""

    var body: some View {
        Group {
            if playlists.playlists.isEmpty {
                KoanUnavailable("No playlists", icon: Icon.playlist, detail: Self.emptyDetail)
            } else {
                VStack(spacing: 0) {
                    NameFilter(text: $filter, prompt: "Filter playlists")
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 80)
                        .padding(.bottom, 16)
                        .focusSection()
                    list
                }
            }
        }
        .navigationTitle(KoanTheme.label("Playlists"))
        .task { playlists.load() }
    }

    private var list: some View {
        List {
            ForEach(shown, id: \.id) { playlist in
                NavigationLink(value: Route.page(.section(.playlist(playlist.id)))) {
                    PlaylistRow(
                        playlist: playlist,
                        covers: playlists.covers[playlist.id] ?? []
                    )
                }
                .listLink()
            }
        }
        .washedGround()
    }

    private var shown: [Playlist] {
        let filter = filter.trimmingCharacters(in: .whitespaces)
        guard !filter.isEmpty else { return playlists.playlists }
        return playlists.playlists.filter { $0.name.localizedStandardContains(filter) }
    }
}
