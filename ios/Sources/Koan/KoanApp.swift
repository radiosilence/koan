import SwiftUI

@main
struct KoanApp: App {
    @AppStorage("serverURL") private var serverURLString = "http://localhost:3000"

    @State private var playbackManager: PlaybackManager?

    var body: some Scene {
        WindowGroup {
            if let manager = playbackManager {
                ContentView()
                    .environment(manager)
            } else {
                ProgressView()
                    .onAppear { createManager() }
            }
        }
        .onChange(of: serverURLString) {
            createManager()
        }
    }

    private func createManager() {
        guard let url = URL(string: serverURLString) else { return }
        playbackManager = PlaybackManager(serverBaseURL: url)
    }
}
