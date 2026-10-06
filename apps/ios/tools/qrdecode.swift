// Prints what each QR code in an image says. `just tv-pair-qr` reads the
// pairing link off the television's screen with it, as a phone's camera would.
//
//     swift apps/ios/tools/qrdecode.swift screenshot.png
import CoreImage
import Foundation

let url = URL(fileURLWithPath: CommandLine.arguments[1])
guard let image = CIImage(contentsOf: url) else {
    fputs("cannot read image\n", stderr)
    exit(1)
}
let detector = CIDetector(
    ofType: CIDetectorTypeQRCode, context: nil,
    options: [CIDetectorAccuracy: CIDetectorAccuracyHigh]
)!
let codes = detector.features(in: image).compactMap { ($0 as? CIQRCodeFeature)?.messageString }
if codes.isEmpty {
    fputs("no QR code found\n", stderr)
    exit(1)
}
codes.forEach { print($0) }
