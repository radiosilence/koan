import KoanFFI
import SwiftUI

/// Where an AI assistant connects to the server: its MCP address, and the
/// server's own page on adding it. Signing the assistant in happens there, in
/// MCP's OAuth, not here.
struct AssistantsSettings: View {
    static let extensionName = "koanMcp"

    @Environment(LibraryModel.self) private var library
    @State private var assistants: Assistants?
    @State private var error: String?
    @State private var copied = false

    var body: some View {
        Section {
            if let assistants {
                LabeledContent("Address", value: assistants.mcpUrl)
                    .selectableText()
                #if !os(tvOS)
                Button {
                    Pasteboard.write(text: assistants.mcpUrl)
                    copied = true
                } label: {
                    KoanLabel(copied ? "Copied" : "Copy Address", icon: "doc.on.doc")
                }
                .koanButton(.secondary)
                .task(id: copied) {
                    guard copied else { return }
                    try? await Task.sleep(for: .seconds(1.5))
                    copied = false
                }
                if let connect = URL(string: assistants.connectUrl) {
                    Link(destination: connect) {
                        KoanLabel("How to Connect an Assistant", icon: "arrow.up.right.square")
                    }
                    .koanButton(.text)
                }
                #endif
            } else if error == nil {
                ProgressView()
            }
        } header: {
            KoanSectionHeader("Assistants")
        } footer: {
            Text(error ?? "Add the address to Claude or another assistant that speaks MCP as a custom connector. It signs in through your server, and can then search your library, make playlists and play music on your devices, as you.")
                .koanText(.fine, error == nil ? .muted : .ink)
        }
        .task {
            do {
                assistants = try await library.engine.assistants()
            } catch {
                self.error = SettingsModel.describe(error)
            }
        }
    }
}
