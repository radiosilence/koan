import KoanFFI
import SwiftUI

/// The signed-in server's accounts, for its admins: add someone, set what they
/// can do, send them an invite, remove them.
///
/// Only a koan server has these, and only an admin may see them; the list
/// failing to load is how both are told apart from a server that has them, so
/// the section simply does not appear.
@MainActor
@Observable
final class PeopleModel {
    private let engine: KoanEngine
    private(set) var accounts: [ServerAccount]?
    var invite: Invite?
    var error: String?
    /// The account waiting for a yes to a new password.
    var resetting: String?

    init(engine: KoanEngine) {
        self.engine = engine
    }

    func load() async {
        accounts = try? await engine.serverAccounts()
    }

    func create(username: String, role: AccountRole) async -> Bool {
        await attempt {
            self.invite = try await self.engine.createServerAccount(username: username, role: role)
        }
    }

    func invite(_ username: String, reset: Bool = false) async {
        _ = await attempt {
            self.invite = try await self.engine.inviteServerAccount(username: username, reset: reset)
        }
    }

    func setPassword(_ username: String, _ password: String) async {
        _ = await attempt {
            try await self.engine.setServerAccountPassword(username: username, password: password)
        }
    }

    func setRole(_ username: String, _ role: AccountRole) async {
        _ = await attempt { try await self.engine.setServerAccountRole(username: username, role: role) }
    }

    func delete(_ username: String) async {
        _ = await attempt { try await self.engine.deleteServerAccount(username: username) }
    }

    /// Runs a change and reloads the list either way, so a refused change
    /// shows what is stored.
    private func attempt(_ work: () async throws -> Void) async -> Bool {
        defer { Task { await load() } }
        do {
            try await work()
            error = nil
            return true
        } catch {
            self.error = SettingsModel.describe(error)
            return false
        }
    }
}

extension AccountRole {
    var label: String {
        switch self {
        case .readonly: "Listen only"
        case .user: "Listen and edit"
        case .admin: "Admin"
        }
    }

    static let all: [AccountRole] = [.readonly, .user, .admin]
}

struct PeopleSettings: View {
    let signedInAs: String
    let model: PeopleModel
    @State private var newUsername = ""
    @State private var newRole = AccountRole.readonly
    @State private var deleting: String?
    @State private var settingPassword: String?
    @State private var password = ""
    @Environment(EngineMirror.self) private var mirror

    var body: some View {
        Group {
            if let accounts = model.accounts {
                Section {
                    ForEach(accounts, id: \.username) { account in
                        row(account, model: model)
                    }
                    HStack {
                        TextField("Username", text: $newUsername, prompt: Text("Username"))
                            .verbatimEntry()
                            .koanField()
                        Picker("Access", selection: $newRole) {
                            ForEach(AccountRole.all, id: \.self) { Text($0.label).tag($0) }
                        }.koanControl()
                        .labelsHidden()
                        .fixedSize()
                        Button("Add") {
                            Task {
                                if await model.create(username: newUsername, role: newRole) {
                                    newUsername = ""
                                }
                            }
                        }
                        .koanButton(.secondary)
                        .disabled(newUsername.trimmingCharacters(in: .whitespaces).isEmpty)
                    }
                    .rowButtons()
                } header: {
                    KoanSectionHeader("People")
                } footer: {
                    Text(model.error ?? "Adding someone makes their invite: one link that sets kōan up with the account.")
                        .koanText(.fine, model.error == nil ? .muted : (KoanTheme.isOn ? .bad : .ink))
                }
                .sheet(item: Binding(
                    get: { model.invite.map(InviteItem.init) },
                    set: { if $0 == nil { model.invite = nil } }
                )) { item in
                    InviteSheet(invite: item.invite)
                }
                .alert(
                    "Give \(model.resetting ?? "") a new password?",
                    isPresented: Binding(
                        get: { model.resetting != nil },
                        set: { if !$0 { model.resetting = nil } }
                    )
                ) {
                    Button("New Password") {
                        if let name = model.resetting { Task { await model.invite(name, reset: true) } }
                    }
                    Button("Cancel", role: .cancel) {}
                } message: {
                    Text("The invite carries the new password. Their devices will have to sign in again.")
                }
                .alert(
                    "Set a password for \(settingPassword ?? "")",
                    isPresented: Binding(
                        get: { settingPassword != nil },
                        set: { if !$0 { settingPassword = nil } }
                    )
                ) {
                    SecureField("New password", text: $password)
                    Button("Set") {
                        if let name = settingPassword {
                            let chosen = password
                            Task { await model.setPassword(name, chosen) }
                        }
                        password = ""
                    }
                    Button("Cancel", role: .cancel) { password = "" }
                } message: {
                    Text("Their devices will have to sign in again with it.")
                }
                .confirmationDialog(
                    "Delete \(deleting ?? "")?",
                    isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } })
                ) {
                    Button("Delete", role: .destructive) {
                        if let name = deleting { Task { await model.delete(name) } }
                    }
                } message: {
                    Text("Their devices stop working, and their playlists and favourites go.")
                }
            }
        }
        .task { await model.load() }
    }

    private func row(_ account: ServerAccount, model: PeopleModel) -> some View {
        HStack {
            Text(account.username)
            if account.username == signedInAs {
                Text("you").koanText(.body, .muted)
            }
            Spacer()
            Picker("Access", selection: Binding(
                get: { account.role },
                set: { role in Task { await model.setRole(account.username, role) } }
            )) {
                ForEach(AccountRole.all, id: \.self) { Text($0.label).tag($0) }
            }.koanControl()
            .labelsHidden()
            .fixedSize()
            Menu {
                Button("Invite") { Task { await model.invite(account.username) } }
                // Not for this account: a new password signs this app out too.
                if account.username != signedInAs {
                    if mirror.offers(PasswordChange.extensionName) {
                        Button("Set Password…") { settingPassword = account.username }
                    }
                    Button("New Password and Invite…") { model.resetting = account.username }
                    Button("Delete", role: .destructive) { deleting = account.username }
                }
            } label: {
                Image(systemName: "ellipsis.circle")
            }.koanControl()
            .menuStyle(.borderlessButton)
            .fixedSize()
            .accessibilityLabel("More for \(account.username)")
        }
    }
}

private struct InviteItem: Identifiable {
    let invite: Invite
    var id: String { invite.link }
}

/// An invite, ready to send. The server sends no mail; this hands the email
/// to whatever the admin sends mail with.
struct InviteSheet: View {
    let invite: Invite
    @Environment(\.dismiss) private var dismiss
    @State private var copied: String?

    var body: some View {
        NavigationStack {
            KoanForm {
                Section {
                    Text("Opening the link on a phone, tablet or Mac with kōan installed signs in and loads the library, on each device, for a week.")
                        .koanText(.body, .muted)
                    #if !os(tvOS)
                    ShareLink(
                        item: invite.emailText,
                        subject: Text(invite.emailSubject),
                        message: Text(invite.emailText)
                    ) {
                        KoanLabel("Send Invite…", icon: "square.and.arrow.up")
                    }
                    .koanButton(.primary)
                    #endif
                    if let mail = URL(string: invite.mailto) {
                        Link(destination: mail) { KoanLabel("Open in Mail", icon: "envelope") }
                            .koanButton(.secondary)
                    }
                    Button {
                        Pasteboard.write(html: invite.emailHtml, text: invite.emailText)
                        copied = "email"
                    } label: {
                        KoanLabel(copied == "email" ? "Copied" : "Copy Email", icon: "doc.on.doc")
                    }
                    .koanButton(.secondary)
                    Button {
                        Pasteboard.write(text: invite.link)
                        copied = "link"
                    } label: {
                        KoanLabel(copied == "link" ? "Copied" : "Copy Link", icon: "link")
                    }
                    .koanButton(.secondary)
                } header: {
                    KoanSectionHeader("Invite for \(invite.username)")
                }
                if let password = invite.password {
                    Section {
                        LabeledContent("Server URL", value: invite.server)
                        LabeledContent("Username", value: invite.username)
                        LabeledContent("Password", value: password)
                    } header: {
                        KoanSectionHeader("For other Subsonic apps")
                    } footer: {
                        Text("Shown this once: the server keeps only its hash.")
                    }
                    .selectableText()
                }
            }
            .koanSheet()
            .navigationTitle(KoanTheme.label("Invite"))
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        #if os(macOS)
        .frame(minWidth: 460, minHeight: 440)
        #endif
    }
}

/// Setting passwords from the app, where the server lists it.
enum PasswordChange {
    static let extensionName = "koanPasswords"
}
