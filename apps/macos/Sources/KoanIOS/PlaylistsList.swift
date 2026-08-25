import SwiftUI

/// The playlists, as a page rather than a sidebar section.
///
/// The Mac lists them beside everything else because it has the room. Here they
/// are a tab, and tapping one navigates to the same `PlaylistView` the Mac
/// shows in its detail column.
struct PlaylistsList: View {
    @Environment(PlaylistsModel.self) private var playlists
    @Environment(Navigator.self) private var nav

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
                    NavigationLink {
                        PlaylistView(playlistId: playlist.id)
                            .environment(\.onStage, true)
                    } label: {
                        PlaylistRow(
                            playlist: playlist,
                            covers: playlists.covers[playlist.id] ?? []
                        )
                    }
                }
            }
        }
        .navigationTitle("Playlists")
        .task { playlists.load() }
    }
}
