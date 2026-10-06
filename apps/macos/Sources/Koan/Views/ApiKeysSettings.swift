import KoanFFI
import SwiftUI

/// The signed-in account's API keys: for other Subsonic apps, and one per
/// device kōan has signed in on, invited ones included. Revoking one is how a
/// lost phone is cut off.
@MainActor
@Observable
final class ApiKeysModel {
    private let engine: KoanEngine
    private(set) var keys: [ApiKeyInfo]?
    /// A key just made, shown until dismissed and then never again.
    var made: NewApiKey?
    var error: String?

    init(engine: KoanEngine) {
        self.engine = engine
    }

    func load() async {
        do {
            keys = try await engine.apiKeys()
        } catch {
            self.error = SettingsModel.describe(error)
        }
    }

    func create(name: String) async -> Bool {
        do {
            made = try await engine.createApiKey(name: name)
            error = nil
            await load()
            return true
        } catch {
            self.error = SettingsModel.describe(error)
            return false
        }
    }

    func revoke(_ key: ApiKeyInfo) async {
        do {
            try await engine.revokeApiKey(id: key.id)
            error = nil
        } catch {
            self.error = SettingsModel.describe(error)
        }
        await load()
    }
}

struct ApiKeysSettings: View {
    static let extensionName = "koanApiKeys"
    @Environment(LibraryModel.self) private var library
    @State private var model: ApiKeysModel?
    @State private var name = ""
    @State private var revoking: ApiKeyInfo?

    var body: some View {
        Section {
            if let model, let keys = model.keys {
                ForEach(keys, id: \.id) { key in
                    row(key)
                }
                HStack {
                    TextField("Name", text: $name, prompt: Text("The app it is for"))
                        .verbatimEntry()
                    Button("New Key") {
                        Task {
                            if await model.create(name: name.trimmingCharacters(in: .whitespaces)) {
                                name = ""
                            }
                        }
                    }
                    .koanButton(.secondary)
                    .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty)
                }
                .rowButtons()
            } else if model?.error == nil {
                ProgressView()
            }
        } header: {
            KoanSectionHeader("API keys")
        } footer: {
            Text(model?.error ?? "A key signs another Subsonic app in as you, without your password. Each device kōan is signed in on has one too; revoking it signs that device out.")
                .koanText(.fine, model?.error == nil ? .muted : .ink)
        }
        .sheet(item: Binding(
            get: { model?.made.map(MadeKey.init) },
            set: { if $0 == nil { model?.made = nil } }
        )) { item in
            NewKeySheet(key: item.key)
        }
        .confirmationDialog(
            "Revoke \u{201C}\(revoking?.name ?? "")\u{201D}?",
            isPresented: Binding(get: { revoking != nil }, set: { if !$0 { revoking = nil } })
        ) {
            Button("Revoke", role: .destructive) {
                if let key = revoking { Task { await model?.revoke(key) } }
            }
        } message: {
            Text("Whatever signs in with it stops working.")
        }
        .task {
            let model = model ?? ApiKeysModel(engine: library.engine)
            self.model = model
            await model.load()
        }
    }

    private func row(_ key: ApiKeyInfo) -> some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text(key.name)
                Text(detail(key))
                    .koanText(.fine, .muted)
            }
            Spacer()
            if key.thisDevice {
                // Revoking it is signing out, which Sign Out does properly.
                Text("This device")
                    .koanText(.body, .muted)
                    .help("To stop using it, sign out")
            } else {
                Button("Revoke", role: .destructive) { revoking = key }
                    .koanButton(.text)
            }
        }
    }

    private func detail(_ key: ApiKeyInfo) -> String {
        let date = { (secs: Int64) in
            Date(timeIntervalSince1970: TimeInterval(secs)).formatted(date: .abbreviated, time: .omitted)
        }
        let made = key.created.map { "Made \(date($0))" }
        let used = key.lastUsed.map { "last used \(date($0))" } ?? "never used"
        return [made, used].compactMap { $0 }.joined(separator: " · ")
    }
}

private struct MadeKey: Identifiable {
    let key: NewApiKey
    var id: String { key.key }
}

/// A key just made: shown this once, since the server keeps only its hash.
private struct NewKeySheet: View {
    let key: NewApiKey
    @Environment(\.dismiss) private var dismiss
    @State private var copied = false

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    LabeledContent("Key", value: key.key)
                        .selectableText()
                    #if !os(tvOS)
                    Button {
                        Pasteboard.write(text: key.key)
                        copied = true
                    } label: {
                        KoanLabel(copied ? "Copied" : "Copy Key", icon: "doc.on.doc")
                    }
                    .koanButton(.secondary)
                    #endif
                } header: {
                    KoanSectionHeader("Key for \(key.name)")
                } footer: {
                    Text("This is the only time the key is shown. Copy it into the app now; if it is lost, revoke it and make another.")
                        .koanText(.fine, .muted)
                }
            }
            .koanForm()
            .koanSheet()
            .navigationTitle(KoanTheme.label("New Key"))
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        #if os(macOS)
        .frame(minWidth: 420, minHeight: 260)
        #endif
    }
}
