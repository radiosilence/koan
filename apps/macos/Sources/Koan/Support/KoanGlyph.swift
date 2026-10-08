import CoreGraphics
#if os(macOS)
import AppKit
#else
import UIKit
#endif

/// One glyph of the kōan icon set: outlines on a 24-unit grid, compiled from
/// `apps/macos/Resources/Icons` by `just icons` into `KoanGlyphs.swift`.
///
/// Strokes are one width at any size, set by `strokeWidth(for:)` rather than
/// scaled with the drawing, with square caps and mitred joins. A glyph may
/// have a fill layer (favourited, selecting); its strokes are then cut out of
/// the fill in one colour, or drawn over it in a second.
/// Immutable once made: its paths are never mutated, so it crosses actors freely.
struct KoanGlyph: @unchecked Sendable {
    static let grid: CGFloat = 24

    let name: String
    /// The SF Symbols this glyph stands in for.
    let symbols: [String]
    let strokes: CGPath
    let fills: CGPath?

    init(name: String, symbols: [String], strokes: String, fills: String? = nil) {
        self.name = name
        self.symbols = symbols
        self.strokes = Self.path(strokes)
        self.fills = fills.map(Self.path)
    }

    /// The glyph standing in for an SF Symbol, if the set has one.
    static func forSymbol(_ symbol: String) -> KoanGlyph? {
        bySymbol[symbol].flatMap { all[$0] }
    }

    /// The square a glyph is drawn in beside type of `pointSize`, the
    /// optical size of an SF Symbol at that point size.
    static func side(for pointSize: CGFloat) -> CGFloat {
        (pointSize * 1.2).rounded()
    }

    /// The stroke, in points, beside type of `pointSize`: 1.2 at 15, 1.4 at
    /// 20, never under a point.
    static func strokeWidth(for pointSize: CGFloat) -> CGFloat {
        max(1, 0.6 + 0.04 * pointSize)
    }

    /// The parts of a glyph to draw: both, or one layer alone for a view
    /// that tints them separately.
    enum Layer: Sendable { case all, fills, strokes }

    /// Draws the glyph into `rect` of a context whose y axis points down, as
    /// a flipped view's does. `fill` is the fill layer's colour; without
    /// one, strokes are cut out of the fill.
    func draw(
        in context: CGContext, rect: CGRect, pointSize: CGFloat, colour: CGColor, fill: CGColor? = nil, layer: Layer = .all
    ) {
        let scale = rect.width / Self.grid
        context.saveGState()
        context.translateBy(x: rect.minX, y: rect.minY)
        context.scaleBy(x: scale, y: scale)
        context.setLineWidth(Self.strokeWidth(for: pointSize) / scale)
        context.setLineCap(.square)
        context.setLineJoin(.miter)
        if let fills, layer != .strokes {
            context.setFillColor(fill ?? colour)
            context.setStrokeColor(fill ?? colour)
            context.addPath(fills)
            context.drawPath(using: .fillStroke)
            if fill == nil, layer == .all { context.setBlendMode(.clear) }
        }
        guard layer != .fills else {
            context.restoreGState()
            return
        }
        context.setStrokeColor(colour)
        context.addPath(strokes)
        context.strokePath()
        context.restoreGState()
    }

    /// Absolute M/L/C/Z commands, as the generator writes them.
    private static func path(_ data: String) -> CGPath {
        let path = CGMutablePath()
        var numbers: [CGFloat] = []
        var command: Character?
        var token = ""

        func flushNumber() {
            if let value = Double(token) { numbers.append(CGFloat(value)) }
            token = ""
        }
        func apply() {
            switch command {
            case "M": path.move(to: CGPoint(x: numbers[0], y: numbers[1]))
            case "L": path.addLine(to: CGPoint(x: numbers[0], y: numbers[1]))
            case "C":
                path.addCurve(
                    to: CGPoint(x: numbers[4], y: numbers[5]),
                    control1: CGPoint(x: numbers[0], y: numbers[1]),
                    control2: CGPoint(x: numbers[2], y: numbers[3])
                )
            case "Z": path.closeSubpath()
            default: break
            }
            numbers.removeAll(keepingCapacity: true)
        }

        for character in data {
            switch character {
            case "M", "L", "C", "Z":
                flushNumber()
                if command != nil { apply() }
                command = character
            case " ":
                flushNumber()
            case "-" where !token.isEmpty:
                flushNumber()
                token.append(character)
            default:
                token.append(character)
            }
        }
        flushNumber()
        if command != nil { apply() }
        return path
    }
}

#if os(macOS)
extension KoanGlyph {
    @MainActor private static var images: [String: NSImage] = [:]

    /// The glyph as a bitmap template image beside type of `pointSize`. A
    /// drawing-handler image reaches an `NSMenu` untinted and black; a bitmap
    /// template is tinted as a symbol is.
    @MainActor
    func image(pointSize: CGFloat, layer: Layer = .all) -> NSImage {
        let key = "\(name) \(pointSize) \(layer)"
        if let held = Self.images[key] { return held }
        let side = Self.side(for: pointSize)
        let scale: CGFloat = 3
        let pixels = Int((side * scale).rounded(.up))
        guard let rep = NSBitmapImageRep(
            bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels, bitsPerSample: 8,
            samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
            bytesPerRow: 0, bitsPerPixel: 0
        ) else { return NSImage(size: NSSize(width: side, height: side)) }
        rep.size = NSSize(width: side, height: side)
        guard let graphics = NSGraphicsContext(bitmapImageRep: rep) else { return NSImage(size: rep.size) }
        let context = graphics.cgContext
        context.translateBy(x: 0, y: side)
        context.scaleBy(x: 1, y: -1)
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = graphics
        draw(in: context, rect: CGRect(x: 0, y: 0, width: side, height: side), pointSize: pointSize, colour: NSColor.black.cgColor, layer: layer)
        NSGraphicsContext.restoreGraphicsState()
        let image = NSImage(size: rep.size)
        image.addRepresentation(rep)
        image.isTemplate = true
        Self.images[key] = image
        return image
    }

    @MainActor private static var dynamicImages: [String: NSImage] = [:]

    /// The glyph for SwiftUI, drawn in the label colour of whatever appearance
    /// it lands in. An `NSMenu` tints only SF Symbols: a template made any
    /// other way, by handler or from a bitmap, reaches it black. SwiftUI views
    /// still tint it by its alpha under `.renderingMode(.template)`; views
    /// that tint an `NSImage` themselves use `image(pointSize:layer:)`.
    @MainActor
    func dynamicImage(pointSize: CGFloat, layer: Layer = .all) -> NSImage {
        let key = "\(name) \(pointSize) \(layer)"
        if let held = Self.dynamicImages[key] { return held }
        let side = Self.side(for: pointSize)
        let image = NSImage(size: NSSize(width: side, height: side), flipped: true) { rect in
            guard let context = NSGraphicsContext.current?.cgContext else { return false }
            draw(in: context, rect: rect, pointSize: pointSize, colour: NSColor.labelColor.cgColor, layer: layer)
            return true
        }
        Self.dynamicImages[key] = image
        return image
    }
}
#else
extension KoanGlyph {
    @MainActor private static var images: [String: UIImage] = [:]

    /// The glyph as a template image beside type of `pointSize`, at the
    /// screen's scale. Tinted where it is used, as a symbol is.
    @MainActor
    func image(pointSize: CGFloat, layer: Layer = .all) -> UIImage {
        let key = "\(name) \(pointSize) \(layer)"
        if let held = Self.images[key] { return held }
        let side = Self.side(for: pointSize)
        let image = UIGraphicsImageRenderer(size: CGSize(width: side, height: side)).image { context in
            draw(in: context.cgContext, rect: CGRect(x: 0, y: 0, width: side, height: side), pointSize: pointSize, colour: UIColor.black.cgColor, layer: layer)
        }
        .withRenderingMode(.alwaysTemplate)
        Self.images[key] = image
        return image
    }
}
#endif
