import KoanFFI
import SwiftUI

/// The signed-in account's app passwords: for Subsonic apps that sign in only
/// with a token and salt, and so cannot use an API key.
@MainActor
@Observable
final class AppPasswordsModel {
    private let engine: KoanEngine
    private(set) var passwords: [AppPasswordInfo]?
    /// A password just made, shown until dismissed and then never again.
    var made: NewAppPassword?
    var error: String?

    init(engine: KoanEngine) {
        self.engine = engine
    }

    func load() async {
        do {
            passwords = try await engine.appPasswords()
        } catch {
            self.error = SettingsModel.describe(error)
        }
    }

    func create(name: String) async -> Bool {
        do {
            made = try await engine.createAppPassword(name: name)
            error = nil
            await load()
            return true
        } catch {
            self.error = SettingsModel.describe(error)
            return false
        }
    }

    func revoke(_ password: AppPasswordInfo) async {
        do {
            try await engine.revokeAppPassword(id: password.id)
            error = nil
        } catch {
            self.error = SettingsModel.describe(error)
        }
        await load()
    }
}

struct AppPasswordsSettings: View {
    static let extensionName = "koanAppPasswords"
    @Environment(LibraryModel.self) private var library
    @State private var model: AppPasswordsModel?
    @State private var name = ""
    @State private var revoking: AppPasswordInfo?

    /// Only on a server that offers them, and only while it can be reached:
    /// anywhere else the section is not there at all.
    static func shown(_ mirror: EngineMirror) -> Bool {
        mirror.offers(extensionName) && mirror.connection?.offline != true
    }

    var body: some View {
        Section {
            if let model, let passwords = model.passwords {
                ForEach(passwords, id: \.id) { password in
                    row(password)
                }
                HStack {
                    TextField("Name", text: $name, prompt: Text("The app it is for"))
                        .verbatimEntry()
                        .koanField(name, prompt: "The app it is for")
                    Button("New App Password") {
                        Task {
                            if await model.create(name: name.trimmingCharacters(in: .whitespaces)) {
                                name = ""
                            }
                        }
                    }
                    .koanButton(.bordered)
                    .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty)
                }
                .rowButtons()
            } else if model?.error == nil {
                ProgressView()
            }
        } header: {
            KoanSectionHeader("App passwords")
        } footer: {
            Text(model?.error ?? "For Subsonic apps that sign in only with a token and salt, and so cannot use an API key. Changing your password revokes them all.")
                .koanText(.fine, model?.error == nil ? .muted : .bad)
        }
        .sheet(item: Binding(
            get: { model?.made.map(MadePassword.init) },
            set: { if $0 == nil { model?.made = nil } }
        )) { item in
            NewSecretSheet(kind: "App Password", name: item.made.name, secret: item.made.password)
        }
        .confirmationDialog(
            "Revoke \u{201C}\(revoking?.name ?? "")\u{201D}?",
            isPresented: Binding(get: { revoking != nil }, set: { if !$0 { revoking = nil } })
        ) {
            Button("Revoke", role: .destructive) {
                if let password = revoking { Task { await model?.revoke(password) } }
            }
        } message: {
            Text("Whatever signs in with it stops working.")
        }
        .task {
            let model = model ?? AppPasswordsModel(engine: library.engine)
            self.model = model
            await model.load()
        }
    }

    private func row(_ password: AppPasswordInfo) -> some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text(password.name)
                Text(detail(password))
                    .koanText(.fine, .muted)
            }
            Spacer()
            Button("Revoke", role: .destructive) { revoking = password }
                .koanButton(.bordered, system: .borderless)
        }
    }

    private func detail(_ password: AppPasswordInfo) -> String {
        let date = { (secs: Int64) in
            Date(timeIntervalSince1970: TimeInterval(secs)).formatted(date: .abbreviated, time: .omitted)
        }
        let made = password.created.map { "Made \(date($0))" }
        let used = password.lastUsed.map { "last used \(date($0))" } ?? "never used"
        return [made, used].compactMap { $0 }.joined(separator: " · ")
    }
}

private struct MadePassword: Identifiable {
    let made: NewAppPassword
    var id: String { made.password }
}
