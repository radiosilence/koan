import SwiftUI

/// Everything you browse, behind one tab.
///
/// The Mac lists albums, artists, favourites, playlists and history side by side
/// in the sidebar, where they cost nothing. As tabs they cost a great deal: past
/// five, iOS folds the rest into More — and More is itself a navigation
/// controller, so a tab that brings its own `NavigationStack` arrives with two
/// back buttons stacked on top of each other.
///
/// One Library tab holds all of them, which leaves the tab bar saying what
/// koan is for: the queue, the library, finding something, and settings.
struct LibraryTab: View {
    @Environment(EngineMirror.self) private var mirror
    @Environment(LibraryModel.self) private var library

    var body: some View {
        List {
            if let connection = mirror.connection, connection.offline {
                offline(manual: connection.offlineManual)
            }
            row("Albums", Icon.album, .page(.section(.albums)))
            row("Artists", Icon.artist, .page(.section(.artists)))
            row("Tracks", Icon.track, .page(.section(.tracks)))
            row("Favourites", Icon.favourite, .page(.section(.favourites)))
            row("Playlists", Icon.playlist, .playlists)
            row("Recently Played", Icon.recentlyPlayed, .page(.section(.recentlyPlayed)))
            row("Downloaded", Icon.onDevice, .page(.section(.onDevice)))
            row("History", Icon.history, .page(.section(.playHistory)))
            row("Downloads", Icon.downloads, .page(.section(.downloads)))
        }
        .navigationTitle("Library")
    }

    /// Offline, and why: by hand, with the way back, or the server out of
    /// reach, which lifts by itself.
    private func offline(manual: Bool) -> some View {
        Section {
            Label(
                manual ? "Offline mode is on" : "Can't reach your server",
                systemImage: "wifi.slash"
            )
            Text("Showing what is on this iPhone.")
                .font(.caption)
                .foregroundStyle(.secondary)
            if manual {
                Button("Go Online") { library.engine.setOffline(on: false) }
            }
        }
    }

    private func row(_ title: String, _ symbol: String, _ route: Route) -> some View {
        NavigationLink(value: route) {
            Label(title, systemImage: symbol)
        }
        .listLink()
    }
}
