import SwiftUI

struct ContentView: View {
    var body: some View {
        TabView {
            NavigationStack {
                LibraryRootView()
            }
            .tabItem { Label("Library", systemImage: "music.note.house") }

            NavigationStack {
                FavouritesView()
            }
            .tabItem { Label("Favourites", systemImage: "heart.fill") }

            NavigationStack {
                SearchView()
            }
            .tabItem { Label("Search", systemImage: "magnifyingglass") }

            NavigationStack {
                ServerSettingsView()
            }
            .tabItem { Label("Settings", systemImage: "gear") }
        }
    }
}
