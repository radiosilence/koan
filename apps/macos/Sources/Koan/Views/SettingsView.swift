import KoanFFI
import SwiftUI

/// Read-mostly. koan's configuration lives in `config.toml` and is shared with
/// the CLI and TUI, so this shows what's in effect and offers the two actions
/// that make sense from a GUI rather than duplicating the file as a form.
struct SettingsView: View {
    @Environment(PlayerModel.self) private var player
    @Environment(LibraryModel.self) private var library

    var body: some View {
        TabView {
            library_
                .tabItem { Label("Library", systemImage: "music.note.house") }
            output
                .tabItem { Label("Output", systemImage: "hifispeaker") }
        }
        .frame(width: 460, height: 300)
    }

    private var library_: some View {
        Form {
            Section("Folders") {
                ForEach(player.engine.libraryFolders(), id: \.self) { folder in
                    Text(folder)
                        .font(.callout.monospaced())
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                }
            }

            Section {
                HStack {
                    Button("Rescan") { library.scan() }
                        .disabled(library.isScanning)
                    Button("Force Rescan") { library.scan(force: true) }
                        .disabled(library.isScanning)
                    if library.isScanning {
                        ProgressView().controlSize(.small)
                    }
                }
                if let summary = library.scanSummary {
                    Text("\(summary.added) added · \(summary.updated) updated · \(summary.removed) removed")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            } header: {
                Text("Scan")
            } footer: {
                Text("Folders are configured in config.toml, shared with the CLI and TUI.")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }
        }
        .formStyle(.grouped)
    }

    private var output: some View {
        Form {
            Section {
                Picker("Output", selection: Binding(
                    get: { player.currentDevice ?? "" },
                    set: { player.setDevice($0.isEmpty ? nil : $0) }
                )) {
                    Text("System Default").tag("")
                    ForEach(player.devices, id: \.name) { device in
                        Text(device.name).tag(device.name)
                    }
                }
            } header: {
                Text("Device")
            } footer: {
                Text("koan switches the device sample rate to match the source. No resampling.")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }
        }
        .formStyle(.grouped)
    }
}
