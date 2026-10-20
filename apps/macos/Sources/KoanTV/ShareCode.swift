import SwiftUI

/// A share link, as a code for a phone to scan.
///
/// A television cannot copy a link or hand it to another app, but everyone in
/// the room has a camera: the link goes up on the screen, and whoever wants it
/// points their phone at it.
struct ShareCode: View {
    let link: String
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        HStack(spacing: 80) {
            QRCodeView(text: link, side: 440, margin: 28, radius: 24)
            VStack(alignment: .leading, spacing: 24) {
                Text("Scan to open").koanCase()
                    .koanText(.titleSmall, .strong)
                Text("Anyone with the link can listen, in a browser or in kōan.")
                    .koanText(.body, .muted)
                Text(link)
                    .koanText(.meta, .muted)
                    .monospaced()
                    .lineLimit(2)
                Button("Done") { dismiss() }
                    .koanButton(.standard)
                    .padding(.top, 24)
            }
            .frame(maxWidth: 640, alignment: .leading)
        }
        .padding(80)
    }

}

extension View {
    /// Shows a share link made on this television as a code to scan. In the
    /// theme, its square panel over the page dimmed, as `televisionPanel`: a
    /// sheet is a rounded card whatever its background is told.
    @ViewBuilder
    func shareCodes(_ player: PlayerModel) -> some View {
        let showing = Binding(
            get: { player.sharedLink != nil },
            set: { if !$0 { player.sharedLink = nil } }
        )
        if KoanTheme.isOn {
            fullScreenCover(isPresented: showing) {
                if let link = player.sharedLink {
                    ShareCode(link: link)
                        .background(Color.koanBg)
                        .overlay { Rectangle().strokeBorder(Color.koanRule, lineWidth: KoanTheme.hairline) }
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        .presentationBackground(Color.black.opacity(0.6))
                }
            }
        } else {
            sheet(isPresented: showing) {
                if let link = player.sharedLink {
                    ShareCode(link: link).koanSheet()
                }
            }
        }
    }
}
