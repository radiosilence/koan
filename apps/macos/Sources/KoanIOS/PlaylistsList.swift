import SwiftUI

/// The playlists, as a page rather than a sidebar section.
///
/// The Mac lists them in its sidebar because it has the room. Here they are a
/// page in the library, and each leads to the same `PlaylistView` the Mac shows.
struct PlaylistsList: View {
    @Environment(PlaylistsModel.self) private var playlists

    var body: some View {
        Group {
            if playlists.playlists.isEmpty {
                ContentUnavailableView(
                    "No playlists",
                    systemImage: Icon.playlist,
                    description: Text("Made here or on your server, they show up in both.")
                )
            } else {
                List(playlists.playlists, id: \.id) { playlist in
                    NavigationLink(value: Route.page(.section(.playlist(playlist.id)))) {
                        PlaylistRow(
                            playlist: playlist,
                            covers: playlists.covers[playlist.id] ?? []
                        )
                    }
                    .listLink()
                }
            }
        }
        .navigationTitle("Playlists")
        #if !os(tvOS)
        // Playlists are made and edited on a phone or a computer; a television
        // plays them.
        .toolbar {
            Button("New Playlist", systemImage: Icon.add) { playlists.naming = [] }
        }
        #endif
        .task { playlists.load() }
    }
}
