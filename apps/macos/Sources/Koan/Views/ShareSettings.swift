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
            LabeledContent("Account", value: account)
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
        Section {
            LabeledContent("Tailscale") {
                if let tailscale = sharing.tailscale {
                    Text("Detected · \(tailscale.name)")
                } else if sharing.tailscaleChecked {
                    Text("Not detected")
                } else {
                    ProgressView().controlSize(.small)
                }
            }
            if let tailscale = sharing.tailscale {
                if let running = sharing.sharing {
                    // Inside the tailnet, WireGuard already encrypts it.
                    LabeledContent("On your tailnet", value: "http://\(tailscale.name):\(running.port)")
                }
                KoanPicker("Invite phones to", selection: Binding(
                    get: { model.settings.shareTailnetInvites },
                    set: { on in
                        model.edit { $0.shareTailnetInvites = on }
                        // The code on screen is for the old address.
                        visits += 1
                    }
                ), options: [("Your tailnet", true), ("This network", false)])
                if let url = tailscale.funnelUrl {
                    LabeledContent("On the internet", value: url)
                    HStack {
                        Button("Stop Publishing") { Task { await sharing.disableFunnel() } }
                            .koanButton(.bordered)
                            .disabled(sharing.busy)
                        Spacer()
                    }
                    .rowButtons()
                } else if tailscale.funnelAllowed {
                    HStack {
                        Button("Publish with Tailscale Funnel…") { confirmingFunnel = true }
                            .koanButton(.bordered)
                            .disabled(sharing.busy || sharing.sharing == nil)
                        Spacer()
                    }
                    .rowButtons()
                } else {
                    Text("Funnel is off for your tailnet. Its admin turns it on in Tailscale's access controls.")
                        .koanText(.meta, .muted)
                }
            } else if sharing.tailscaleChecked {
                Link("Get Tailscale", destination: URL(string: "https://tailscale.com/download")!)
            }
            if let funnelError = sharing.funnelError {
                KoanLabel(funnelError, icon: Icon.warning).koanText(.meta, .bad)
            }
            LabeledContent("Public address") {
                TextField("Public address", text: Binding(
                    get: { publicAddress ?? model.settings.serverUrl },
                    set: { publicAddress = $0 }
                ), prompt: Text("https://mac.example.ts.net"))
                .verbatimEntry(.url)
                .onSubmit(commitPublicAddress)
                .koanField(publicAddress ?? model.settings.serverUrl, prompt: "https://mac.example.ts.net")
            }
        } header: {
            KoanSectionHeader("Away from home")
        } footer: {
            Text(sharing.tailscale == nil
                ? "Tailscale on this Mac and your phone reaches it from anywhere, encrypted, and can publish it on the internet. Otherwise a tunnel's HTTPS address goes under Public address, and invites send phones there."
                : "With Tailscale on your phone too, it reaches this Mac from anywhere, encrypted, and nothing is on the internet. Funnel publishes it on the internet, over HTTPS, for phones without Tailscale. A public address, when set, is where invites go.")
                .koanText(.fine, .muted)
        }
        .task { await sharing.refreshTailscale() }
        .confirmationDialog(
            "Publish kōan on the internet?",
            isPresented: $confirmingFunnel,
            titleVisibility: .visible
        ) {
            Button("Publish") { Task { await sharing.enableFunnel() } }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Tailscale Funnel publishes this Mac at https://\(sharing.tailscale?.name ?? "") with a certificate of its own, and keeps it published whenever this Mac shares. Anyone can reach it; only accounts made here can sign in. Invites will send phones there.")
        }
        // The engine wrote the address Funnel publishes at.
        .onChange(of: sharing.revision) { model.reload() }
    }

    private func setSharing(_ on: Bool) {
        model.edit { $0.shareEnabled = on }
        let (port, address) = (model.settings.sharePort, model.settings.serverUrl)
        Task {
            if on {
                await app.sharing.start(port: port, publicAddress: address)
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

    /// Read when the server starts, for the names it answers to.
    private func commitPublicAddress() {
        guard let typed = publicAddress else { return }
        publicAddress = nil
        let address = typed.trimmingCharacters(in: .whitespaces)
        guard address != model.settings.serverUrl else { return }
        if !address.isEmpty, !(address.hasPrefix("https://") || address.hasPrefix("http://")) {
            model.report("A public address starts with https://.")
            return
        }
        model.edit { $0.serverUrl = address }
        // The address is this Mac's tailnet name, which only Funnel serves:
        // offered as the way to make it work.
        if let tailscale = app.sharing.tailscale, tailscale.funnelUrl == nil, tailscale.funnelAllowed,
           URL(string: address)?.host() == tailscale.name {
            confirmingFunnel = true
        }
        restartIfSharing()
    }

    /// With the values just typed, so the start never reads a setting still
    /// being written.
    private func restartIfSharing() {
        guard model.settings.shareEnabled else { return }
        let (port, address) = (model.settings.sharePort, model.settings.serverUrl)
        Task {
            await app.sharing.start(port: port, publicAddress: address)
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
