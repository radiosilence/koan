import AppKit
import SwiftUI

/// koan's accent, read from the asset catalog.
///
/// The catalog is the source of truth rather than a literal here, because
/// AppKit needs it there: list selection, focus rings and control tints come
/// from the app's declared accent colour, and nothing in SwiftUI can override
/// them. `.tint` reaches SwiftUI's own drawing and stops at the edge of every
/// AppKit-backed control, which is why the sidebar stayed blue however the app
/// was tinted.
///
/// `NSAccentColorName` in the bundle's Info.plist points at this colour set;
/// see the `macos-bundle` recipe.
/// Falls back to the system accent when the catalog is not in the bundle.
/// Compiling it needs `actool`, which ships with Xcode proper rather than the
/// command line tools, so a build made without Xcode has no colour to find.
/// Resolving to nothing tints the app with nothing, which does not merely lose
/// the colour — every borderless control and the playing row's title are drawn
/// in `.tint`, and they become invisible rather than uncoloured.
extension Color {
    static let koanAccent = NSColor(named: "AccentColor").map(Color.init) ?? .accentColor
}


extension Color {
    /// The colour a record reads as, for tinting the app while it plays.
    ///
    /// Not the average of the sleeve: averaging every pixel of a busy cover
    /// gives the same brown-grey every time, because opposite hues cancel. This
    /// is a circular mean of *hue* weighted by how colourful each sample is, so
    /// the one strong colour on a mostly black cover wins rather than being
    /// drowned by the black.
    ///
    /// The result is forced into a band that stays legible as a tint on dark
    /// chrome. A navy sleeve would otherwise give an accent invisible against
    /// the window and a neon one would flare.
    static func dominant(of image: NSImage) -> Color? {
        let side = 12
        var pixels = [UInt8](repeating: 0, count: side * side * 4)
        var rect = NSRect(origin: .zero, size: image.size)
        guard let cgImage = image.cgImage(forProposedRect: &rect, context: nil, hints: nil),
              let context = CGContext(
                  data: &pixels,
                  width: side,
                  height: side,
                  bitsPerComponent: 8,
                  bytesPerRow: side * 4,
                  space: CGColorSpaceCreateDeviceRGB(),
                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
              )
        else { return nil }
        context.draw(cgImage, in: CGRect(x: 0, y: 0, width: side, height: side))

        var x = 0.0, y = 0.0, saturation = 0.0, total = 0.0
        for i in stride(from: 0, to: pixels.count, by: 4) {
            let (hue, sat, value) = hsb(
                Double(pixels[i]) / 255,
                Double(pixels[i + 1]) / 255,
                Double(pixels[i + 2]) / 255
            )
            // Near-grey, near-black and blown-out samples say nothing about
            // what colour the record is.
            guard sat > 0.15, value > 0.15, value < 0.98 else { continue }
            let weight = sat * value
            let angle = hue * 2 * .pi
            x += cos(angle) * weight
            y += sin(angle) * weight
            saturation += sat * weight
            total += weight
        }
        guard total > 0 else { return nil }

        let mean = atan2(y / total, x / total) / (2 * .pi)
        return Color(
            hue: mean < 0 ? mean + 1 : mean,
            saturation: min(0.85, max(0.55, saturation / total)),
            brightness: 0.80
        )
    }

    /// Written out rather than going through `NSColor`, whose HSB components
    /// are only valid in colour spaces it will happily hand you a colour
    /// outside of.
    private static func hsb(_ r: Double, _ g: Double, _ b: Double) -> (Double, Double, Double) {
        let high = max(r, g, b), low = min(r, g, b), delta = high - low
        guard delta > 0 else { return (0, 0, high) }
        var hue: Double
        if high == r {
            hue = ((g - b) / delta).truncatingRemainder(dividingBy: 6)
        } else if high == g {
            hue = (b - r) / delta + 2
        } else {
            hue = (r - g) / delta + 4
        }
        hue /= 6
        return (hue < 0 ? hue + 1 : hue, high == 0 ? 0 : delta / high, high)
    }
}
