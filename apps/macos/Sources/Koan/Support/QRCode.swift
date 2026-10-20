import CoreImage.CIFilterBuiltins
import SwiftUI

/// A link as a QR code, for a phone to scan off the screen: dark modules on a
/// white ground whatever the theme, since scanners expect that contrast. Each
/// module is a whole number of pixels, so edges stay crisp; the code fits
/// within `side`. Nothing is drawn, caption included, for text too long to
/// encode.
struct QRCodeView: View {
    let text: String
    let side: CGFloat
    let margin: CGFloat
    let radius: CGFloat
    var caption: String?
    @State private var code: CGImage?
    @Environment(\.displayScale) private var scale

    var body: some View {
        VStack(spacing: 8) {
            if let code {
                let pixels = max(1, (side * scale / CGFloat(code.width)).rounded(.down))
                let fitted = pixels * CGFloat(code.width) / scale
                Image(decorative: code, scale: 1)
                    .interpolation(.none)
                    .resizable()
                    .frame(width: fitted, height: fitted)
                    .padding(margin)
                    .background(.white, in: .rect(cornerRadius: KoanTheme.radius(radius))) // theme: raw — a QR code needs a white ground to scan
                    .accessibilityElement()
                    .accessibilityLabel("QR code")
                if let caption {
                    Text(caption).koanText(.meta, .muted)
                }
            }
        }
        .task(id: text) { code = qrImage(text) }
    }
}

@MainActor private let qrContext = CIContext()

/// `text` as a QR code, one pixel to a module: drawn with interpolation off,
/// it scales to any size without blurring.
@MainActor
private func qrImage(_ text: String) -> CGImage? {
    let filter = CIFilter.qrCodeGenerator()
    filter.message = Data(text.utf8)
    filter.correctionLevel = "M"
    guard let image = filter.outputImage else { return nil }
    return qrContext.createCGImage(image, from: image.extent)
}
