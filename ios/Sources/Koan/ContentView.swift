import SwiftUI

struct ContentView: View {
    var body: some View {
        TabView {
            NavigationStack {
                ArtistListView()
            }
            .tabItem {
                Label("Library", systemImage: "music.note.list")
            }

            NavigationStack {
                ServerSettingsView()
            }
            .tabItem {
                Label("Settings", systemImage: "gear")
            }
        }
    }
}
