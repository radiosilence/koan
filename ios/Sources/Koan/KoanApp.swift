import SwiftUI

@main
struct KoanApp: App {
    @State private var serverConfig = ServerConfig()

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environment(serverConfig)
        }
    }
}
