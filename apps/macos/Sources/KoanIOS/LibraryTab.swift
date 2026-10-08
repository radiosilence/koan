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

    var body: some View {
        List {
            if LibraryStatus.showing(mirror) {
                Section { LibraryStatus() }
            }
            row("Albums", Icon.album, .page(.section(.albums)))
            row("Artists", Icon.artist, .page(.section(.artists)))
            // No Tracks: the whole library as one list is tens of thousands of
            // rows a phone or a television draws and plays from at once. The
            // track browser is reached filtered, from a shelf's or a search's
            // "See all"; the Mac, which draws only the rows on screen, keeps it.
            row("Favourites", Icon.favourite, .page(.section(.favourites)))
            row("Playlists", Icon.playlist, .playlists)
            row("Recently Played", Icon.recentlyPlayed, .page(.section(.recentlyPlayed)))
            row("Downloaded", Icon.onDevice, .page(.section(.onDevice)))
            row("History", Icon.history, .page(.section(.playHistory)))
            row("Downloads", Icon.downloads, .page(.section(.downloads)))
        }
        .koanList()
        .navigationTitle(KoanTheme.tabRootTitle("Library"))
    }

    private func row(_ title: String, _ symbol: String, _ route: Route) -> some View {
        NavigationLink(value: route) {
            KoanLabel(title, icon: symbol, style: .row)
        }
        .listLink()
    }
}

/// Why the library shows less than it holds: the server refused the sign-in,
/// or koan is offline, by hand, with the way back, or with the server out of
/// reach, which lifts by itself. Atop the Library tab, and the iPad's sidebar.
struct LibraryStatus: View {
    @Environment(EngineMirror.self) private var mirror
    @Environment(LibraryModel.self) private var library

    static func showing(_ mirror: EngineMirror) -> Bool {
        mirror.signInRefused || mirror.connection?.offline == true
    }

    var body: some View {
        if mirror.signInRefused {
            Label(EngineMirror.signInRefusedDetail, systemImage: "exclamationmark.triangle")
                .foregroundStyle(KoanTheme.style(.bad, system: .orange))
        } else if let connection = mirror.connection, connection.offline {
            Label(
                connection.offlineManual ? "Offline mode is on" : "Can't reach your server",
                systemImage: "wifi.slash"
            )
            Text("Showing what is on this \(Self.device).")
                .font(.role(.fine, system: .caption))
                .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
            if connection.offlineManual {
                Button("Go Online") { library.engine.setOffline(on: false) }
            }
        }
    }

    private static var device: String {
        #if os(tvOS)
        "Apple TV"
        #else
        UIDevice.current.userInterfaceIdiom == .pad ? "iPad" : "iPhone"
        #endif
    }
}
