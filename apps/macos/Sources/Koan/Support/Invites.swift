import KoanFFI
import SwiftUI

/// Invites: a link that carries an account, opened in one tap.
///
/// The link carries a token, which is traded for an API key of this device's
/// own. The link
/// arrives as a universal link (`koan.rocks/join/`), as `koan://join`
/// from the join page, or pasted into Settings. Opening it signs in, syncs the
/// whole library and shows it, with nothing asked — except when it would
/// replace an account already signed in, which is not a thing to do to
/// someone unasked.
extension AppState {
    /// Whatever the system opened koan with. Anything but an invite or a
    /// pairing link (see `Pairing.swift`) is ignored.
    func open(url: URL) {
        if let link = engine.parsePairingLink(link: url.absoluteString) {
            Task { await offer(link) }
            return
        }
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
                try await engine.joinInvite(invite: invite)
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
                try await engine.syncRemote()
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
///
/// Handed the state rather than reading it from the environment: it is applied
/// at the scene root, outside the `.environment(state)` it would read from, and
/// a missing environment object is a crash on launch rather than a build error.
struct InviteConfirmation: ViewModifier {
    let state: AppState

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
