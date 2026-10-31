import KoanFFI
import SwiftUI

/// A QR code that signs another device in as this account: the account's own
/// invite, which every account may have. Its token lapses in ten minutes, so a
/// fresh one is fetched each time the section appears and again before it
/// lapses. A server that refuses (one older than self-invites, to an account
/// that is not an admin) is not asked again for that account. One that
/// could not be reached is asked once more half a minute later, then again
/// when the connection to it changes or the page is shown again. The section
/// is not drawn while the server refuses, nor after the retry fails.
struct LinkDeviceSection: View {
    let username: String
    /// Moves each time the page holding this is shown again.
    let visit: Int
    @Environment(LibraryModel.self) private var library
    @Environment(EngineMirror.self) private var mirror
    @State private var invite: Invite?
    /// The account the server refused an invite for.
    @State private var refused: String?
    /// The ask that could not reach the server, until the connection changes.
    @State private var unreached: Ask?

    private struct Ask: Equatable {
        let username: String
        let visit: Int
        let linked: Bool
        let offline: Bool
    }

    private var ask: Ask {
        Ask(
            username: username,
            visit: visit,
            linked: mirror.connection?.linked == true,
            offline: mirror.connection?.offline == true
        )
    }

    /// How long a failure to reach the server waits before the one retry it
    /// gets; after that, only a change of connection or a visit asks again.
    private static let retryAfter = Duration.seconds(30)

    var body: some View {
        if refused != username, unreached != ask { section }
    }

    private var section: some View {
        Section {
            if let invite {
                QRCodeView(
                    text: invite.qrLink, side: 220, margin: 14, radius: 10,
                    caption: "Scan with your phone's camera to sign it in as \(username)."
                )
                .frame(maxWidth: .infinity)
                #if !os(tvOS)
                ShareLink(item: invite.qrLink) {
                    KoanLabel("Share Link…", icon: Icon.export)
                }
                .koanButton(.bordered)
                .frame(maxWidth: .infinity)
                #endif
            } else {
                ProgressView().frame(maxWidth: .infinity)
            }
        } header: {
            KoanSectionHeader("Link a Device")
        } footer: {
            Text("kōan opens on the other device and loads the library, or offers the download first.")
                .koanText(.fine, .muted)
        }
        .task(id: ask) {
            let asked = ask
            var retried = false
            while !Task.isCancelled {
                do {
                    invite = try await library.engine.inviteServerAccount(username: username, reset: false)
                    retried = false
                } catch KoanError.Refused {
                    invite = nil
                    refused = asked.username
                    return
                } catch {
                    invite = nil
                    guard !retried else {
                        unreached = asked
                        return
                    }
                    retried = true
                    try? await Task.sleep(for: Self.retryAfter)
                    continue
                }
                // Fetched again before the token lapses at ten minutes.
                try? await Task.sleep(for: .seconds(9 * 60))
            }
        }
    }
}
