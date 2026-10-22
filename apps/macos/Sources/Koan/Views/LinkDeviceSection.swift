import KoanFFI
import SwiftUI

/// A QR code that signs another device in as this account: the account's own
/// invite, which every account may have. Its token lapses in ten minutes, so a
/// fresh one is fetched each time the section appears and again before it
/// lapses. While the server refuses or cannot be reached the section is not
/// drawn, and asks again a minute later: a server older than self-invites
/// refuses an account that is not an admin.
struct LinkDeviceSection: View {
    let username: String
    @Environment(LibraryModel.self) private var library
    @State private var invite: Invite?
    @State private var failed = false

    var body: some View {
        if !failed { section }
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
        .task(id: username) {
            while !Task.isCancelled {
                let made = try? await library.engine.inviteServerAccount(username: username, reset: false)
                invite = made
                failed = made == nil
                try? await Task.sleep(for: .seconds(made == nil ? 60 : 9 * 60))
            }
        }
    }
}
