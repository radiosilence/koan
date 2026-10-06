import KoanFFI
import SwiftUI

/// Signing a television in without typing a password.
///
/// The TV opens a pairing on the server and shows it as a code and a QR code;
/// someone signed in on a phone or a Mac approves it, and the server hands
/// this TV a key of its own on their account. The server is usually found
/// rather than typed: kōan on a phone or Mac on the same network announces the
/// server it is signed in to, and the page offers each one it hears of for as
/// long as it is open. Typing the address is the fallback, and the phone's
/// keyboard can do that. A server that is not koan cannot pair, so the account
/// form stays a click away for those.
struct SignInPage: View {
    let signedIn: () -> Void

    @Environment(AppState.self) private var state
    @Environment(ActivityModel.self) private var activity
    @Environment(EngineMirror.self) private var mirror
    @State private var server = ""
    /// A pairing being opened: the server can take a minute to answer, and a
    /// second press meanwhile would race the first for the engine's one slot.
    @State private var connecting = false
    @State private var pairing: PairingCode?
    @State private var waiting: Task<Void, Never>?
    @State private var problem: String?
    @State private var manual = false

    var body: some View {
        VStack(spacing: 48) {
            VStack(spacing: 16) {
                Text(KoanTheme.label("Sign in to kōan"))
                    .koanText(.display, .strong)
                Text(pairing == nil
                     ? "Open kōan on a device that is signed in to your server and on this network. Its server will appear here."
                     : "Scan with your phone's camera. It opens kōan if it is there, or \(host)'s own page if not.")
                    .koanText(.body, .muted)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: 1400)
            }

            if let pairing {
                waitingCard(pairing)
            } else {
                found
                addressForm
            }

            if let problem {
                Text(problem)
                    .koanText(.body, .bad)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 1200)
            }

            HStack(spacing: 32) {
                if pairing != nil {
                    Button("Another Server") { cancel() }
                        .koanButton(.secondary)
                }
                Button("Use a Password or API Key") { manual = true }
                    .koanButton(.secondary)
            }
        }
        .padding(80)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background {
            ZStack {
                Rectangle().fill(.black)
                RadialGradient(
                    colors: [Color.koanAccent.opacity(0.18), .clear],
                    center: UnitPoint(x: 0.2, y: 0.1),
                    startRadius: 0,
                    endRadius: 1200
                )
            }
            .ignoresSafeArea()
        }
        .task {
            let settings = await state.engine.settings()
            if server.isEmpty { server = settings.remoteUrl }
        }
        .onDisappear { cancel() }
        // The whole screen, as the Settings tab has it: a sheet on a
        // television is a narrow card, too small for a form read from across
        // the room. Menu closes it.
        .fullScreenCover(isPresented: $manual, onDismiss: recheck) {
            NavigationStack { SettingsView() }
                .background(Color.black.ignoresSafeArea())
        }
        // The form's sign-in runs as an activity, and can finish after the
        // cover has been closed.
        .onChange(of: activity.tasks.count) { recheck() }
    }

    /// The servers found so far, each a press away from a code, and the search
    /// that goes on finding them.
    private var found: some View {
        let servers = mirror.connection?.nearbyServers ?? []
        return VStack(spacing: 24) {
            ForEach(servers, id: \.url) { found in
                Button { choose(found.url) } label: {
                    VStack(spacing: 6) {
                        Text(Self.address(found.url))
                            .koanText(.titleSmall, .strong)
                        Text("On \(ListFormatter.localizedString(byJoining: found.devices))")
                            .koanText(.meta, .muted)
                    }
                    .frame(minWidth: 700)
                    .padding(.vertical, 8)
                }
                .koanButton(.secondary)
                .disabled(connecting)
                .accessibilityIdentifier("found-server")
            }
            if mirror.connection?.localNetworkBlocked == true {
                Text("kōan can't see this network. Allow Local Network for kōan in \(LocalNetwork.settings) → Privacy & Security.")
                    .koanText(.body, .bad)
                    .multilineTextAlignment(.center)
            } else {
                HStack(spacing: 16) {
                    ProgressView()
                    Text(servers.isEmpty ? "Looking for kōan on this network…" : "Still looking for others…")
                        .koanText(.body, .muted)
                }
            }
            Text(servers.isEmpty ? "Or enter your server's address" : "Or enter its address")
                .koanText(.meta, .muted)
                .padding(.top, 16)
        }
    }

    /// An address as a person reads it: without the scheme, or a slash at the end.
    private static func address(_ url: String) -> String {
        var s = url
        for scheme in ["https://", "http://"] where s.hasPrefix(scheme) {
            s.removeFirst(scheme.count)
        }
        return s.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
    }

    private func choose(_ url: String) {
        server = url
        start()
    }

    private var addressForm: some View {
        HStack(spacing: 24) {
            TextField("https://music.example.com", text: $server)
                .accessibilityIdentifier("pair-server")
                .keyboardType(.URL)
                .textContentType(.URL)
                .autocorrectionDisabled()
                .frame(width: 900)
                .onSubmit(start)
                .disabled(connecting)
            if connecting {
                ProgressView()
                    .frame(width: 240)
            } else {
                Button("Get a Code", action: start)
                    .koanButton(.primary)
                    .disabled(server.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
    }

    private func waitingCard(_ pairing: PairingCode) -> some View {
        HStack(spacing: 80) {
            if let code = qrImage(pairing.link) {
                Image(decorative: code, scale: 1)
                    .interpolation(.none)
                    .resizable()
                    .frame(width: 400, height: 400)
                    .padding(24)
                    .background(.white, in: .rect(cornerRadius: KoanTheme.radius(24))) // theme: raw — a QR code needs a white ground to scan
            }
            VStack(alignment: .leading, spacing: 20) {
                Text(host)
                    .koanText(.titleSmall, .strong)
                Text("Or enter this code under Settings → Server → Pair a device, or at \(host)/pair:")
                    .koanText(.body, .muted)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: 640, alignment: .leading)
                Text(pairing.code)
                    .koanText(.display, .strong)
                    .monospaced()
                    .tracking(8)
                HStack(spacing: 16) {
                    ProgressView()
                    Text("Waiting for approval")
                        .koanText(.body, .muted)
                }
            }
        }
        .padding(56)
        .koanSurface(.surface)
    }

    private var host: String {
        URL(string: normalised)?.host() ?? normalised
    }

    /// What was typed, as an address: a bare host is taken as https.
    private var normalised: String {
        let typed = server.trimmingCharacters(in: .whitespaces)
        return typed.contains("://") ? typed : "https://\(typed)"
    }

    private func start() {
        guard !connecting else { return }
        let url = normalised
        let engine = state.engine
        problem = nil
        connecting = true
        waiting?.cancel()
        waiting = Task {
            do {
                let opened = try await engine.startPairing(url: url)
                connecting = false
                // Cancelling the task does not interrupt the call already in
                // flight; a pairing opened after "Another Server" is given up.
                guard !Task.isCancelled else {
                    engine.cancelPairing()
                    return
                }
                pairing = opened
                try await engine.awaitPairing()
                pairing = nil
                signedIn()
            } catch {
                connecting = false
                guard !Task.isCancelled else { return }
                pairing = nil
                problem = Self.explain(error)
            }
        }
    }

    private func recheck() {
        let engine = state.engine
        Task { if await engine.settings().remoteSignedIn { signedIn() } }
    }

    private func cancel() {
        state.engine.cancelPairing()
        waiting?.cancel()
        waiting = nil
        pairing = nil
    }

    private static func explain(_ error: Error) -> String {
        let reason = SettingsModel.describe(error)
        // Only a server without pairing needs the other way in pointed out;
        // a declined or lapsed code is asked for again.
        if case KoanError.NotFound = error {
            return "\(reason.prefix(1).uppercased() + reason.dropFirst()). A server that is not kōan signs in with a password or API key."
        }
        return "Could not sign in this way: \(reason)."
    }
}
