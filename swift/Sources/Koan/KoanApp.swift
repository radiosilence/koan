import SwiftUI

@main
struct KoanApp: App {
    var body: some Scene {
        WindowGroup {
            ContentView()
        }
        .windowStyle(.titleBar)
        .defaultSize(width: 1000, height: 700)
    }
}
