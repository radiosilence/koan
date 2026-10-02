#if os(macOS)
import AppKit

/// SF Symbols drawn once into bitmaps for layers, which take no part in
/// layout or hit-testing.
@MainActor
enum Symbol {
    private static var cache: [String: CGImage] = [:]

    /// Drawn in `appearance`, which dynamic colours such as
    /// `tertiaryLabelColor` resolve against.
    static func image(
        _ name: String, size: CGFloat, weight: NSFont.Weight = .regular, colours: [NSColor],
        appearance: NSAppearance = .currentDrawing()
    ) -> CGImage? {
        let key = "\(name) \(size) \(weight.rawValue) \(colours.map(\.description)) \(appearance.name.rawValue)"
        if let held = cache[key] { return held }
        // One colour is the symbol in that colour, as SwiftUI's foreground
        // style draws it: what is cut out of it stays cut out. A palette of
        // one paints every layer, filling the play mark's triangle in.
        var configuration = NSImage.SymbolConfiguration(pointSize: size, weight: weight)
        if colours.count > 1 { configuration = configuration.applying(.init(paletteColors: colours)) }
        guard let symbol = NSImage(systemSymbolName: name, accessibilityDescription: nil)?
            .withSymbolConfiguration(configuration),
            let bitmap = NSBitmapImageRep(
                bitmapDataPlanes: nil,
                pixelsWide: Int(ceil(symbol.size.width * 2)), pixelsHigh: Int(ceil(symbol.size.height * 2)),
                bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0
            )
        else { return nil }
        bitmap.size = symbol.size
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
        appearance.performAsCurrentDrawingAppearance {
            let rect = NSRect(origin: .zero, size: symbol.size)
            symbol.draw(in: rect)
            // The colour replaces the glyph's, keeping only its shape. Laid
            // over it instead, a translucent colour — every label colour but
            // the first — would be tinted by the black glyph underneath.
            if colours.count == 1 {
                colours[0].set()
                rect.fill(using: .sourceIn)
            }
        }
        NSGraphicsContext.restoreGraphicsState()
        cache[key] = bitmap.cgImage
        return bitmap.cgImage
    }

    /// Where a symbol drawn by `image` sits at its natural size: it is drawn
    /// at 2×, so half its pixels.
    static func frame(of image: CGImage?, centredIn rect: CGRect) -> CGRect {
        let size = size(of: image)
        return CGRect(x: rect.midX - size.width / 2, y: rect.midY - size.height / 2, width: size.width, height: size.height)
    }

    static func frame(of image: CGImage?, at origin: CGPoint) -> CGRect {
        CGRect(origin: origin, size: size(of: image))
    }

    private static func size(of image: CGImage?) -> CGSize {
        guard let image else { return .zero }
        return CGSize(width: CGFloat(image.width) / 2, height: CGFloat(image.height) / 2)
    }
}
#endif
