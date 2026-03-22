import SwiftUI

struct ContentView: View {
    @Environment(PlaybackManager.self) private var playback
    @State private var showNowPlaying = false

    var body: some View {
        ZStack(alignment: .bottom) {
            NavigationStack {
                List {
                    NavigationLink {
                        ServerSettingsView()
                    } label: {
                        Label("Server", systemImage: "server.rack")
                    }
                }
                .navigationTitle("koan")
            }

            // Mini player pinned to bottom (above tab bar area)
            if playback.nowPlaying.track != nil {
                MiniPlayerView {
                    showNowPlaying = true
                }
            }
        }
        #if os(iOS)
        .fullScreenCover(isPresented: $showNowPlaying) {
            NowPlayingView()
                .environment(playback)
        }
        #else
        .sheet(isPresented: $showNowPlaying) {
            NowPlayingView()
                .environment(playback)
        }
        #endif
        .onAppear {
            playback.startPolling()
        }
    }
}
