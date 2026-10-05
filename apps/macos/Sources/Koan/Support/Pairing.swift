import KoanFFI
import SwiftUI

/// Pairing: signing in a device that has no keyboard, such as a television.
///
/// The device shows a code and a link (`koan.rocks/pair/`). The link arrives
/// here as invites do; the code is typed in Settings. Either way this app asks
/// the server what the device calls itself, asks the person, and on a yes the
/// server signs the device in as the account signed in here, with an API key
/// of its own. Nothing of this device's credentials leaves it.
struct PairingRequest: Equatable {
    /// The pairing's id or code, as the server knows it.
    let pair: String
    let device: String
    let username: String
    let host: String
}

extension AppState {
    /// A pairing link opened from outside. Only for the server signed in
    /// here: approving signs the device in to this account.
    func offer(_ link: PairingLink) async {
        let current = await engine.settings()
        let host = URL(string: link.server)?.host() ?? link.server
        let signedIn = current.remoteUrl.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        guard current.remoteSignedIn, signedIn == link.server else {
            player.lastNotice = "A device is asking to sign in to \(host), which kōan is not signed in to. Approve it on \(host)'s own pair page instead."
            return
        }
        await offerPairing(link.id)
    }

    /// Ask the signed-in server which device is waiting on `pair`, an id or a
    /// typed code, and put the question to the person.
    func offerPairing(_ pair: String) async {
        let current = await engine.settings()
        let host = URL(string: current.remoteUrl)?.host() ?? current.remoteUrl
        do {
            let device = try await engine.pairingInfo(pair: pair)
            ui.pendingPairing = PairingRequest(
                pair: pair, device: device, username: current.remoteUsername, host: host
            )
        } catch {
            player.report("No device is waiting with that code: \(SettingsModel.describe(error))")
        }
    }

    func settle(_ request: PairingRequest, approve: Bool) {
        ui.pendingPairing = nil
        let engine = self.engine
        Task {
            do {
                if approve {
                    let device = try await engine.approvePairing(pair: request.pair)
                    player.lastNotice = "\(device) is signed in as \(request.username)."
                } else {
                    try await engine.declinePairing(pair: request.pair)
                }
            } catch {
                player.report("Could not sign \(request.device) in: \(SettingsModel.describe(error))")
            }
        }
    }
}

/// "Sign in this device?", for a pairing link or a typed code.
///
/// Handed the state rather than reading it from the environment, as
/// `InviteConfirmation` is, for the same reason.
struct PairingConfirmation: ViewModifier {
    let state: AppState

    func body(content: Content) -> some View {
        content.alert(
            "Sign in \(state.ui.pendingPairing?.device ?? "this device")?",
            isPresented: Binding(
                get: { state.ui.pendingPairing != nil },
                set: { if !$0 { state.ui.pendingPairing = nil } }
            ),
            presenting: state.ui.pendingPairing
        ) { request in
            Button("Allow") { state.settle(request, approve: true) }
            Button("Decline", role: .cancel) { state.settle(request, approve: false) }
        } message: { request in
            Text(
                "It will be signed in as \(request.username) on \(request.host). Allow only a device you are setting up yourself: anyone can give a device any name."
            )
        }
    }
}
