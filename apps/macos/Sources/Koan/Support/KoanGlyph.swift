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

    /// Draws the glyph into `rect` of a context whose y axis points down, as
    /// a flipped view's does. `fill` is the fill layer's colour; without
    /// one, strokes are cut out of the fill.
    func draw(in context: CGContext, rect: CGRect, pointSize: CGFloat, colour: CGColor, fill: CGColor? = nil) {
        let scale = rect.width / Self.grid
        context.saveGState()
        context.translateBy(x: rect.minX, y: rect.minY)
        context.scaleBy(x: scale, y: scale)
        context.setLineWidth(Self.strokeWidth(for: pointSize) / scale)
        context.setLineCap(.square)
        context.setLineJoin(.miter)
        if let fills {
            context.setFillColor(fill ?? colour)
            context.setStrokeColor(fill ?? colour)
            context.addPath(fills)
            context.drawPath(using: .fillStroke)
            if fill == nil { context.setBlendMode(.clear) }
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

    /// The glyph as a template image beside type of `pointSize`, drawn at
    /// whatever resolution it lands on. Tinted where it is used, as a
    /// symbol is.
    @MainActor
    func image(pointSize: CGFloat) -> NSImage {
        let key = "\(name) \(pointSize)"
        if let held = Self.images[key] { return held }
        let side = Self.side(for: pointSize)
        let image = NSImage(size: NSSize(width: side, height: side), flipped: true) { rect in
            guard let context = NSGraphicsContext.current?.cgContext else { return false }
            draw(in: context, rect: rect, pointSize: pointSize, colour: NSColor.black.cgColor)
            return true
        }
        image.isTemplate = true
        Self.images[key] = image
        return image
    }
}
#else
extension KoanGlyph {
    @MainActor private static var images: [String: UIImage] = [:]

    /// The glyph as a template image beside type of `pointSize`, at the
    /// screen's scale. Tinted where it is used, as a symbol is.
    @MainActor
    func image(pointSize: CGFloat) -> UIImage {
        let key = "\(name) \(pointSize)"
        if let held = Self.images[key] { return held }
        let side = Self.side(for: pointSize)
        let image = UIGraphicsImageRenderer(size: CGSize(width: side, height: side)).image { context in
            draw(in: context.cgContext, rect: CGRect(x: 0, y: 0, width: side, height: side), pointSize: pointSize, colour: UIColor.black.cgColor)
        }
        .withRenderingMode(.alwaysTemplate)
        Self.images[key] = image
        return image
    }
}
#endif
