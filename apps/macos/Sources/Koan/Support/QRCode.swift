import CoreImage.CIFilterBuiltins
import SwiftUI

/// A link as a QR code, for a phone to scan off the screen: dark modules on a
/// white ground whatever the theme, since scanners expect that contrast.
struct QRCodeView: View {
    let text: String
    let side: CGFloat
    let margin: CGFloat
    let radius: CGFloat

    var body: some View {
        if let code = qrImage(text) {
            Image(decorative: code, scale: 1)
                .interpolation(.none)
                .resizable()
                .frame(width: side, height: side)
                .padding(margin)
                .background(.white, in: .rect(cornerRadius: KoanTheme.radius(radius))) // theme: raw — a QR code needs a white ground to scan
                .accessibilityElement()
                .accessibilityLabel("QR code")
        }
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
