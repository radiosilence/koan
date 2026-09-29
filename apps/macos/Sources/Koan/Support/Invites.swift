import KoanFFI
import SwiftUI

/// Invites: a link that carries an account, opened in one tap.
///
/// The link arrives as a universal link (`koan.rocks/join/`), as `koan://join`
/// from the join page, or pasted into Settings. Opening it signs in, syncs the
/// whole library and shows it, with nothing asked — except when it would
/// replace an account already signed in, which is not a thing to do to
/// someone unasked.
extension AppState {
    /// Whatever the system opened koan with. Anything but an invite is ignored.
    func open(url: URL) {
        guard let invite = engine.parseInvite(link: url.absoluteString) else { return }
        Task { await offer(invite) }
    }

    func offer(_ invite: Invite) async {
        let current = await engine.settings()
        let elsewhere = current.remoteUrl != invite.server || current.remoteUsername != invite.username
        if current.remoteSignedIn && elsewhere {
            ui.pendingInvite = invite
        } else {
            join(invite)
        }
    }

    func join(_ invite: Invite) {
        ui.pendingInvite = nil
        let engine = self.engine
        let host = Self.host(of: invite)
        Task {
            let signedIn = await activity.run("Signing in to \(host)") {
                try await engine.signInRemote(
                    url: invite.server, username: invite.username, password: invite.password)
            }
            if case .failure(let error) = signedIn {
                player.lastError = "Could not sign in to \(host): \(SettingsModel.describe(error))"
                return
            }
            nav.show(.albums)
            #if os(iOS)
            PushDelegate.requestAlertsIfSignedIn()
            #endif
            let synced = await activity.run(
                "Loading the library", uses: [.remoteTracks], followsSync: true
            ) {
                try await engine.syncRemote(full: true)
            }
            if case .failure(let error) = synced {
                player.lastError = SettingsModel.describe(error)
            }
        }
    }

    static func host(of invite: Invite) -> String {
        URL(string: invite.server)?.host() ?? invite.server
    }
}

/// The one question an invite asks: whether to leave the account already
/// signed in. Tracks synced from it stay, as they do on signing out.
struct InviteConfirmation: ViewModifier {
    @Environment(AppState.self) private var state

    func body(content: Content) -> some View {
        content.alert(
            "Switch accounts?",
            isPresented: Binding(
                get: { state.ui.pendingInvite != nil },
                set: { if !$0 { state.ui.pendingInvite = nil } }
            ),
            presenting: state.ui.pendingInvite
        ) { invite in
            Button("Switch") { state.join(invite) }
            Button("Cancel", role: .cancel) {}
        } message: { invite in
            Text(
                "kōan is signed in to another account. This invite signs in as \(invite.username) on \(AppState.host(of: invite)); tracks already synced stay in the library."
            )
        }
    }
}
