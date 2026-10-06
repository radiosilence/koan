import CoreImage.CIFilterBuiltins
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
            if let code = qrImage(link) {
                Image(decorative: code, scale: 1)
                    .interpolation(.none)
                    .resizable()
                    .frame(width: 440, height: 440)
                    .padding(28)
                    .background(.white, in: .rect(cornerRadius: KoanTheme.radius(24))) // theme: raw — a QR code needs a white ground to scan
            }
            VStack(alignment: .leading, spacing: 24) {
                Text(KoanTheme.label("Scan to open"))
                    .koanText(.titleSmall, .strong)
                Text("Anyone with the link can listen, in a browser or in kōan.")
                    .koanText(.body, .muted)
                Text(link)
                    .koanText(.meta, .muted)
                    .monospaced()
                    .lineLimit(2)
                Button("Done") { dismiss() }
                    .koanButton(.secondary)
                    .padding(.top, 24)
            }
            .frame(maxWidth: 640, alignment: .leading)
        }
        .padding(80)
        .koanSheet()
    }

}

/// `text` as a QR code, one pixel to a module: drawn with interpolation off,
/// it scales to any size without blurring.
func qrImage(_ text: String) -> CGImage? {
    let filter = CIFilter.qrCodeGenerator()
    filter.message = Data(text.utf8)
    filter.correctionLevel = "M"
    guard let image = filter.outputImage else { return nil }
    return CIContext().createCGImage(image, from: image.extent)
}

extension View {
    /// Shows a share link made on this television as a code to scan.
    func shareCodes(_ player: PlayerModel) -> some View {
        sheet(isPresented: Binding(
            get: { player.sharedLink != nil },
            set: { if !$0 { player.sharedLink = nil } }
        )) {
            if let link = player.sharedLink {
                ShareCode(link: link)
            }
        }
    }
}
