// The kōan icon set on one page, for review: every glyph beside the SF Symbol
// it replaces, at 17 and 13 points, each at 2× and at 1× (enlarged pixel for
// pixel), and large, on the theme's light and dark grounds.
//
// Built with KoanGlyph.swift and KoanGlyphs.swift by `just icons-sheet`, so it
// draws exactly what the apps draw.
//
// Usage: icons-sheet OUTPUT.png FONT.woff2

import AppKit
import CoreText

let arguments = CommandLine.arguments
guard arguments.count == 3 else {
    FileHandle.standardError.write("usage: icons-sheet OUTPUT.png FONT.woff2\n".data(using: .utf8)!)
    exit(2)
}
CTFontManagerRegisterFontsForURL(URL(fileURLWithPath: arguments[2]) as CFURL, .process, nil)

struct Ground {
    let name: String
    let bg: CGColor
    let ink: CGColor
    let muted: CGColor
    let rule: CGColor
}

func hex(_ value: UInt32) -> CGColor {
    CGColor(
        srgbRed: CGFloat((value >> 16) & 0xff) / 255, green: CGFloat((value >> 8) & 0xff) / 255,
        blue: CGFloat(value & 0xff) / 255, alpha: 1
    )
}

let grounds = [
    Ground(name: "light", bg: hex(0xffffff), ink: hex(0x333333), muted: hex(0x666666), rule: hex(0xe0e0e0)),
    Ground(name: "dark", bg: hex(0x1e1e1e), ink: hex(0xcccccc), muted: hex(0x919191), rule: hex(0x383838)),
]

// Filled states sit beside the stroke glyphs they belong to.
var names = KoanGlyph.all.keys.sorted()
for (filled, beside) in [("selecting", "select"), ("heart-filled", "heart")] {
    names.removeAll { $0 == filled }
    names.insert(filled, at: names.firstIndex(of: beside)! + 1)
}

let scale: CGFloat = 2
let columns = 4
let cellWidth: CGFloat = 300
let rowHeight: CGFloat = 56
let labelHeight: CGFloat = 34
let cellHeight = labelHeight + rowHeight * CGFloat(grounds.count)
let margin: CGFloat = 24
let header: CGFloat = 64
let rows = (names.count + columns - 1) / columns
let width = margin * 2 + cellWidth * CGFloat(columns)
let height = header + margin + cellHeight * CGFloat(rows)

let space = CGColorSpace(name: CGColorSpace.sRGB)!
func bitmap(_ w: CGFloat, _ h: CGFloat, scale: CGFloat) -> CGContext {
    let context = CGContext(
        data: nil, width: Int((w * scale).rounded(.up)), height: Int((h * scale).rounded(.up)),
        bitsPerComponent: 8, bytesPerRow: 0, space: space,
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    )!
    context.scaleBy(x: scale, y: scale)
    return context
}

let sheet = bitmap(width, height, scale: scale)
// Top-left origin, y down, for layout; text and images flip back locally.
sheet.translateBy(x: 0, y: height)
sheet.scaleBy(x: 1, y: -1)

func font(_ size: CGFloat) -> CTFont {
    CTFontCreateWithName("Geist Mono" as CFString, size, nil)
}

func text(_ string: String, at point: CGPoint, size: CGFloat, colour: CGColor) {
    let attributes: [NSAttributedString.Key: Any] = [
        NSAttributedString.Key(kCTFontAttributeName as String): font(size),
        NSAttributedString.Key(kCTForegroundColorAttributeName as String): colour,
    ]
    let line = CTLineCreateWithAttributedString(NSAttributedString(string: string, attributes: attributes))
    sheet.saveGState()
    sheet.translateBy(x: point.x, y: point.y)
    sheet.scaleBy(x: 1, y: -1)
    sheet.textPosition = .zero
    CTLineDraw(line, sheet)
    sheet.restoreGState()
}

func place(_ image: CGImage, in rect: CGRect, smooth: Bool) {
    sheet.saveGState()
    sheet.interpolationQuality = smooth ? .high : .none
    sheet.translateBy(x: rect.minX, y: rect.maxY)
    sheet.scaleBy(x: 1, y: -1)
    sheet.draw(image, in: CGRect(origin: .zero, size: rect.size))
    sheet.restoreGState()
}

/// A glyph drawn alone at `pixelScale`, as the apps draw it.
func glyphImage(_ glyph: KoanGlyph, pointSize: CGFloat, colour: CGColor, pixelScale: CGFloat) -> CGImage {
    let side = KoanGlyph.side(for: pointSize)
    let context = bitmap(side, side, scale: pixelScale)
    context.translateBy(x: 0, y: side)
    context.scaleBy(x: 1, y: -1)
    glyph.draw(in: context, rect: CGRect(x: 0, y: 0, width: side, height: side), pointSize: pointSize, colour: colour)
    return context.makeImage()!
}

/// The SF Symbol at `pointSize`, light, in one colour, as the theme drew it before.
func symbolImage(_ name: String, pointSize: CGFloat, colour: CGColor) -> (CGImage, CGSize)? {
    guard let symbol = NSImage(systemSymbolName: name, accessibilityDescription: nil)?
        .withSymbolConfiguration(.init(pointSize: pointSize, weight: .light))
    else { return nil }
    let size = symbol.size
    let context = bitmap(size.width, size.height, scale: scale)
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(cgContext: context, flipped: false)
    let rect = NSRect(origin: .zero, size: size)
    symbol.draw(in: rect)
    NSColor(cgColor: colour)!.set()
    rect.fill(using: .sourceIn)
    NSGraphicsContext.restoreGraphicsState()
    return (context.makeImage()!, size)
}

sheet.setFillColor(grounds[0].bg)
sheet.fill(CGRect(x: 0, y: 0, width: width, height: height))
text("kōan icon set: \(names.count) glyphs", at: CGPoint(x: margin, y: 30), size: 15, colour: grounds[0].ink)
text(
    "each row: SF 17 · kōan 17 @2x · 17 @1x · 13 @2x · 13 @1x · 40",
    at: CGPoint(x: margin, y: 50), size: 10, colour: grounds[0].muted
)

for (index, name) in names.enumerated() {
    let glyph = KoanGlyph.all[name]!
    let origin = CGPoint(
        x: margin + CGFloat(index % columns) * cellWidth,
        y: header + margin + CGFloat(index / columns) * cellHeight
    )
    text(name, at: CGPoint(x: origin.x + 8, y: origin.y + 14), size: 11, colour: grounds[0].ink)
    text(
        glyph.symbols.joined(separator: " "), at: CGPoint(x: origin.x + 8, y: origin.y + 28),
        size: 8, colour: grounds[0].muted
    )
    for (row, ground) in grounds.enumerated() {
        let top = origin.y + labelHeight + CGFloat(row) * rowHeight
        let band = CGRect(x: origin.x, y: top, width: cellWidth - 8, height: rowHeight)
        sheet.setFillColor(ground.bg)
        sheet.fill(band)
        sheet.setStrokeColor(ground.rule)
        sheet.setLineWidth(1 / scale)
        sheet.stroke(band)
        let mid = top + rowHeight / 2
        var x = origin.x + 8
        if let (image, size) = symbolImage(glyph.symbols.first ?? "", pointSize: 17, colour: ground.muted) {
            place(image, in: CGRect(x: x + (24 - size.width) / 2, y: mid - size.height / 2, width: size.width, height: size.height), smooth: true)
        }
        x += 34
        for (pointSize, pixelScale) in [(CGFloat(17), scale), (17, 1), (13, scale), (13, 1)] {
            let side = KoanGlyph.side(for: pointSize)
            let image = glyphImage(glyph, pointSize: pointSize, colour: ground.ink, pixelScale: pixelScale)
            place(image, in: CGRect(x: x, y: mid - side / 2, width: side, height: side), smooth: pixelScale == scale)
            x += side + 12
        }
        let large = KoanGlyph.side(for: 40)
        let image = glyphImage(glyph, pointSize: 40, colour: ground.ink, pixelScale: scale)
        place(image, in: CGRect(x: band.maxX - large - 4, y: mid - large / 2, width: large, height: large), smooth: true)
    }
}

let image = sheet.makeImage()!
let destination = CGImageDestinationCreateWithURL(
    URL(fileURLWithPath: arguments[1]) as CFURL, "public.png" as CFString, 1, nil
)!
CGImageDestinationAddImage(destination, image, [kCGImagePropertyDPIWidth: 144, kCGImagePropertyDPIHeight: 144] as CFDictionary)
CGImageDestinationFinalize(destination)
print("\(names.count) glyphs → \(arguments[1])")
