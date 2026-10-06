import KoanFFI
import SwiftUI

/// Signing a television in without typing a password.
///
/// The TV opens a pairing on the server and shows it as a code and a QR code;
/// someone signed in on a phone or a Mac approves it, and the server hands
/// this TV a key of its own on their account. Typing the server's address is
/// the only typing, and the phone's keyboard can do that. A server that is not
/// koan cannot pair, so the account form stays a click away for those.
struct SignInPage: View {
    let signedIn: () -> Void

    @Environment(AppState.self) private var state
    @Environment(ActivityModel.self) private var activity
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
                Text("Sign in to kōan")
                    .font(.system(size: 64, weight: .bold))
                Text(pairing == nil
                     ? "Enter your server's address. The keyboard on your phone can type it."
                     : "Scan with a phone signed in to \(host), or approve it in kōan on a Mac.")
                    .font(.title3)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
            }

            if let pairing {
                waitingCard(pairing)
            } else {
                addressForm
            }

            if let problem {
                Text(problem)
                    .foregroundStyle(.orange)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 1200)
            }

            HStack(spacing: 32) {
                if pairing != nil {
                    Button("Another Server") { cancel() }
                }
                Button("Use a Password or API Key") { manual = true }
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
        .sheet(isPresented: $manual, onDismiss: recheck) {
            NavigationStack { SettingsView() }
        }
        // The form's sign-in runs as an activity, and can finish after the
        // sheet has been dismissed.
        .onChange(of: activity.tasks.count) { recheck() }
    }

    private var addressForm: some View {
        HStack(spacing: 24) {
            TextField("https://music.example.com", text: $server)
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
                    .background(.white, in: .rect(cornerRadius: 24))
            }
            VStack(alignment: .leading, spacing: 20) {
                Text(host)
                    .font(.title2.weight(.semibold))
                Text("Or enter this code under Settings → Server → Pair a device, or at \(host)/pair:")
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: 640, alignment: .leading)
                Text(pairing.code)
                    .font(.system(size: 72, weight: .semibold, design: .monospaced))
                    .tracking(8)
                HStack(spacing: 16) {
                    ProgressView()
                    Text("Waiting for approval")
                        .foregroundStyle(.secondary)
                }
            }
        }
        .padding(56)
        .background(.white.opacity(0.06), in: .rect(cornerRadius: 32))
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
