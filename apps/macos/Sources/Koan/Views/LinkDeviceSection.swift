import KoanFFI
import SwiftUI

/// A QR code that signs another device in as this account: the account's own
/// invite, which every account may have. Its token lapses in ten minutes, so a
/// fresh one is fetched each time the section appears and again before it
/// lapses. While the server refuses or cannot be reached the section is not
/// drawn, and asks again when the link to the server comes or goes, or the
/// page is opened again: a server older than self-invites refuses an account
/// that is not an admin.
struct LinkDeviceSection: View {
    let username: String
    @Environment(LibraryModel.self) private var library
    @Environment(EngineMirror.self) private var mirror
    @State private var invite: Invite?
    /// The ask that failed: the section stays away until the ask changes.
    @State private var failed: Ask?

    private struct Ask: Equatable {
        let username: String
        let linked: Bool
    }

    private var ask: Ask { Ask(username: username, linked: mirror.connection?.linked == true) }

    var body: some View {
        if failed != ask { section }
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
            while !Task.isCancelled {
                guard let made = try? await library.engine.inviteServerAccount(username: username, reset: false)
                else {
                    invite = nil
                    failed = asked
                    return
                }
                invite = made
                // Fetched again before the token lapses at ten minutes.
                try? await Task.sleep(for: .seconds(9 * 60))
            }
        }
    }
}
