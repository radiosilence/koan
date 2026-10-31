#if os(macOS)
import KoanFFI
import SwiftUI

/// What this Mac does about servers: signs in to one, or is one.
enum ServerMode: Hashable {
    case connect, share
}

/// The choice at the top of Settings → Server, and a line saying what this
/// Mac is doing now, whichever side is showing.
struct ServerModeSection: View {
    @Bindable var model: SettingsModel
    @Binding var mode: ServerMode
    @Environment(AppState.self) private var app

    var body: some View {
        Section {
            KoanPicker("This Mac", selection: $mode, options: [
                ("Connects to a server", .connect), ("Shares its library", .share),
            ])
            ServerModeLabel()
                .koanText(.meta, .muted)
        } footer: {
            Text(mode == .share
                ? "Your phone and other devices play this Mac's library, with an account you make here."
                : "This Mac plays a kōan, Navidrome or Subsonic server's library, alongside its own folders.")
                .koanText(.fine, .muted)
        }
    }
}

/// What this Mac is doing now, as one line: the same words in Settings and at
/// the foot of the sidebar.
struct ServerModeLabel: View {
    @Environment(AppState.self) private var app
    @Environment(EngineMirror.self) private var mirror

    var body: some View {
        let sharing = app.sharing
        if let running = sharing.sharing {
            KoanLabel("Sharing on port \(running.port)", icon: Icon.server)
        } else if let failure = sharing.failure {
            KoanLabel("Not sharing: \(failure)", icon: Icon.warning)
                .foregroundStyle(KoanTheme.style(.bad, system: .orange))
        } else if let server = mirror.connection?.signedInTo {
            KoanLabel("Connected to \(Self.host(server))", icon: Icon.server)
        } else {
            KoanLabel("Playing this Mac's own library", icon: Icon.library)
        }
    }

    static func host(_ url: String) -> String {
        URL(string: url)?.host() ?? url
    }
}

/// Settings → Server → Shares its library.
struct ShareSettings: View {
    @Bindable var model: SettingsModel
    @Environment(AppState.self) private var app
    @State private var username = ""
    @State private var password = ""
    @State private var confirmation = ""
    @State private var creating = false
    @State private var accountError: String?
    /// The port and public address as typed, committed whole.
    @State private var port: String?
    @State private var publicAddress: String?
    @State private var copied = false
    @State private var visits = 0
    /// Asking before the library goes on the internet.
    @State private var confirmingFunnel = false
    @State private var resettingPassword = false
    @State private var newPassword = ""
    @State private var newPasswordAgain = ""

    var body: some View {
        let sharing = app.sharing
        Group {
        if model.settings.remoteSignedIn {
            Section {
                Text("This Mac is signed in to \(ServerModeLabel.host(model.settings.remoteUrl)). Sign out of it under Connects to a server, then share this Mac's own library here.")
                    .koanText(.body, .muted)
            }
        } else if let account = sharing.account {
            running(sharing, account: account)
        } else {
            createAccount
        }
        }
        // The address moves with the network, so it is worked out again each
        // time the page opens.
        .task { await sharing.refresh() }
    }

    // MARK: First account

    private var createAccount: some View {
        Section {
            LabeledContent("Username") {
                TextField("Username", text: $username, prompt: Text("Username"))
                    .verbatimEntry()
                    .koanField(username, prompt: "Username")
            }
            LabeledContent("Password") {
                SecureField("Password", text: $password, prompt: Text("Password"))
                    .koanField(password, prompt: "Password", secure: true)
            }
            LabeledContent("Again") {
                SecureField("Again", text: $confirmation, prompt: Text("Password again"))
                    .koanField(confirmation, prompt: "Password again", secure: true)
            }
            if let accountError {
                KoanLabel(accountError, icon: Icon.warning).koanText(.meta, .bad)
            }
            HStack {
                Button("Create Account and Share") { create() }
                    .koanButton(.prominent)
                    .disabled(creating || username.trimmingCharacters(in: .whitespaces).isEmpty
                        || password.isEmpty || password != confirmation)
                Spacer()
            }
            .rowButtons()
        } header: {
            KoanSectionHeader("Your account")
        } footer: {
            Text("Your phone signs in with this account. It is the server's admin: it invites other people and approves devices.")
                .koanText(.fine, .muted)
        }
    }

    private func create() {
        creating = true
        let (name, secret) = (username, password)
        Task {
            defer { creating = false }
            if let reason = await app.sharing.createAccount(username: name, password: secret) {
                accountError = reason
                return
            }
            password = ""
            confirmation = ""
            accountError = nil
            setSharing(true)
        }
    }

    // MARK: Sharing

    @ViewBuilder
    private func running(_ sharing: SharingModel, account: String) -> some View {
        Section {
            Toggle("Share this Mac's library", isOn: Binding(
                get: { model.settings.shareEnabled },
                set: { setSharing($0) }
            ))
            .koanToggle()
            .disabled(sharing.busy)
            if let running = sharing.sharing, let url = running.lanUrl {
                // What a phone on this network is pointed at, so a click copies it.
                Button {
                    Pasteboard.write(text: url)
                    copied = true
                } label: {
                    LabeledContent("On your network", value: copied ? "Copied" : url)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help("Copy the address")
                .task(id: copied) {
                    guard copied else { return }
                    try? await Task.sleep(for: .seconds(1.5))
                    copied = false
                }
            }
            if let failure = sharing.failure {
                KoanLabel(failure, icon: Icon.warning).koanText(.meta, .bad)
            }
            LabeledContent("Account") {
                HStack(spacing: KoanTheme.Space.l) {
                    Text(account)
                    Button("Reset Password…") { resettingPassword = true }
                        .koanButton(.compact, system: .bordered)
                }
            }
            .alert("New password for \(account)", isPresented: $resettingPassword) {
                SecureField("New password", text: $newPassword)
                SecureField("Again", text: $newPasswordAgain)
                Button("Reset") {
                    let password = newPassword
                    Task {
                        if let reason = await app.sharing.resetPassword(password) {
                            model.report(reason)
                        } else {
                            model.done("\(account)'s password is changed. Its other devices sign in again.")
                        }
                    }
                }
                .disabled(newPassword.isEmpty || newPassword != newPasswordAgain)
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("Its phones and other devices are signed out and sign in again with the new one.")
            }
            // However the alert closes, what was typed goes.
            .onChange(of: resettingPassword) { _, open in
                if !open {
                    newPassword = ""
                    newPasswordAgain = ""
                }
            }
            LabeledContent("Port") {
                TextField("Port", text: Binding(
                    get: { port ?? String(model.settings.sharePort) },
                    set: { port = $0 }
                ), prompt: Text("4000"))
                .monospacedDigit()
                .onSubmit(commitPort)
                .koanField(port ?? String(model.settings.sharePort), prompt: "4000")
            }
        } header: {
            KoanSectionHeader("Share this Mac")
        } footer: {
            Text("Devices on your network reach this Mac while kōan is open and the Mac is awake. Sign-ins and music cross your network unencrypted, as with any plain http:// server.")
                .koanText(.fine, .muted)
        }

        if model.settings.shareEnabled {
            awayFromHome(sharing)
        }

        if sharing.sharing != nil {
            LinkDeviceSection(username: account, visit: visits)
                .onAppear { visits += 1 }
        }
    }

    // MARK: Away from home

    @ViewBuilder
    private func awayFromHome(_ sharing: SharingModel) -> some View {
        // From the engine's last reading, with the address beside it, so the
        // two never show different moments.
        let choice = sharing.status?.to ?? model.settings.shareInviteTo
        let serving = sharing.sharing != nil
        Section {
            LabeledContent("Tailscale") {
                if let tailscale = sharing.tailscale {
                    Text("Detected · \(tailscale.name)")
                } else if sharing.tailscaleChecked {
                    Link("Not detected · Get Tailscale", destination: URL(string: "https://tailscale.com/download")!)
                } else {
                    ProgressView().controlSize(.small)
                }
            }
            KoanPicker("Invite phones to", selection: Binding(
                get: { choice },
                set: { to in
                    guard to != choice else { return }
                    if to == .funnel {
                        confirmingFunnel = true
                    } else {
                        Task { await sharing.setInviteTo(to) }
                    }
                }
            ), options: inviteOptions(sharing, choice: choice, serving: serving))
            .disabled(sharing.busy)
            if serving, let addresses = sharing.status {
                if choice == .custom {
                    LabeledContent("Address") {
                        TextField("Address", text: Binding(
                            get: { publicAddress ?? model.settings.shareUrl },
                            set: { publicAddress = $0 }
                        ), prompt: Text(addresses.fallback ?? ""))
                        .verbatimEntry(.url)
                        .onSubmit(commitPublicAddress)
                        .koanField(publicAddress ?? model.settings.shareUrl, prompt: addresses.fallback ?? "")
                    }
                } else {
                    LabeledContent("Address", value: addresses.current ?? "")
                }
                if choice == .funnel, addresses.funnel == nil {
                    KoanLabel("Not published on the internet yet: invites use the address above.", icon: Icon.warning)
                        .koanText(.meta, .bad)
                }
            }
            if let funnelError = sharing.funnelError {
                KoanLabel(funnelError, icon: Icon.warning).koanText(.meta, .bad)
            }
        } header: {
            KoanSectionHeader("Away from home")
        } footer: {
            Text(inviteNote(choice))
                .koanText(.fine, .muted)
        }
        .task { await sharing.refreshStatus() }
        .confirmationDialog(
            "Publish kōan on the internet?",
            isPresented: $confirmingFunnel,
            titleVisibility: .visible
        ) {
            Button("Publish") { Task { await sharing.setInviteTo(.funnel) } }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Tailscale Funnel publishes this Mac at https://\(sharing.tailscale?.name ?? "") with a certificate of its own, and keeps it published whenever this Mac shares. Anyone can reach it; only accounts made here can sign in.")
        }
        // The engine wrote where invites go; Settings reads it again rather
        // than keeping its older copy.
        .onChange(of: sharing.revision) { model.reload() }
    }

    private func inviteOptions(_ sharing: SharingModel, choice: InviteTo, serving: Bool) -> [(label: String, value: InviteTo)] {
        var options: [(label: String, value: InviteTo)] = [("This network", .network)]
        if sharing.tailscale != nil || choice == .tailnet {
            let missing = sharing.tailscale == nil && sharing.tailscaleChecked
            options.append((missing ? "Your tailnet (Tailscale not running)" : "Your tailnet", .tailnet))
        }
        // Publishing needs a server to publish.
        if (sharing.tailscale?.funnelAllowed == true && serving) || choice == .funnel {
            options.append(("Tailscale Funnel", .funnel))
        }
        options.append(("Custom address", .custom))
        return options
    }

    private func inviteNote(_ choice: InviteTo) -> String {
        switch choice {
        case .network: "Phones reach this Mac on your network only."
        case .tailnet: "Phones with Tailscale reach this Mac from anywhere, encrypted, and nothing is on the internet."
        case .funnel: "On the internet over HTTPS, for phones without Tailscale. Taken down when this Mac stops sharing, and put back when it starts."
        case .custom: "A tunnel's HTTPS address, such as Cloudflare Tunnel's. Empty, invites use the address shown."
        }
    }

    private func setSharing(_ on: Bool) {
        model.edit { $0.shareEnabled = on }
        let port = model.settings.sharePort
        Task {
            if on {
                await app.sharing.start(port: port)
                // Left on, the switch would read as sharing beside the reason
                // it is not. Launch still retries a setting left on.
                if app.sharing.failure != nil { model.edit { $0.shareEnabled = false } }
            } else {
                await app.sharing.stop()
            }
        }
    }

    /// A server already running moves to the new port.
    private func commitPort() {
        guard let typed = port else { return }
        port = nil
        guard let value = UInt16(typed.trimmingCharacters(in: .whitespaces)), value > 0 else {
            model.report("A port is a number from 1 to 65535.")
            return
        }
        guard value != model.settings.sharePort else { return }
        model.edit { $0.sharePort = value }
        restartIfSharing()
    }

    /// Through the engine, which owns where invites go.
    private func commitPublicAddress() {
        guard let typed = publicAddress else { return }
        publicAddress = nil
        let address = typed.trimmingCharacters(in: .whitespaces)
        guard address != model.settings.shareUrl else { return }
        Task {
            await app.sharing.setInviteTo(.custom, address: address)
            // The server answers to the names it was started with, the
            // custom address's among them.
            if app.sharing.funnelError == nil { restartIfSharing() }
        }
    }

    /// With the values just typed, so the start never reads a setting still
    /// being written.
    private func restartIfSharing() {
        guard model.settings.shareEnabled else { return }
        let port = model.settings.sharePort
        Task {
            await app.sharing.start(port: port)
            // A start stops the running server first, so a failed one leaves
            // nothing serving: the switch says so, as it does when turned on.
            if app.sharing.failure != nil { model.edit { $0.shareEnabled = false } }
        }
    }
}

/// What this Mac is doing about servers, at the foot of the sidebar. A click
/// opens Settings → Server.
struct SidebarServerMode: View {
    @Environment(AppState.self) private var app
    @Environment(\.openSettings) private var openSettings

    var body: some View {
        Button {
            app.ui.showingServerSettings = true
            openSettings()
        } label: {
            ServerModeLabel()
                .lineLimit(2)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .koanText(.meta, .muted)
        .help("Server settings")
    }
}
#endif
