// Draws the DMG window's background at 1x and 2x.
//
//   swift background.swift <AppIcon.svg> <geist-mono.woff2> <out-dir>
//
// The ensō is the app icon's own stroke, read from its SVG, so the two cannot
// drift apart. The layout constants must match `settings.py`: the window size,
// and the icon centres the brushstroke runs between.

import AppKit
import CoreText

let args = CommandLine.arguments
guard args.count == 4 else {
    FileHandle.standardError.write("usage: background.swift <AppIcon.svg> <font.woff2> <out-dir>\n".data(using: .utf8)!)
    exit(2)
}
let (svgPath, fontPath, outDir) = (args[1], args[2], args[3])

let size = CGSize(width: 660, height: 400)
let app = CGPoint(x: 170, y: 190)
let applications = CGPoint(x: 490, y: 190)

// koan.rocks' dark palette.
let ground = CGColor(srgbRed: 0x1E / 255, green: 0x1E / 255, blue: 0x1E / 255, alpha: 1)
let ink = CGColor(srgbRed: 0xCC / 255, green: 0xCC / 255, blue: 0xCC / 255, alpha: 1)
let brand = CGColor(srgbRed: 0xE6 / 255, green: 0x5E / 255, blue: 0x5E / 255, alpha: 1)
let muted = CGColor(srgbRed: 0x8C / 255, green: 0x8C / 255, blue: 0x8C / 255, alpha: 1)

// Finder draws icon labels in black over a background image, whatever the
// appearance. Each label gets a light plate to sit on; its centre is where
// Finder puts the label for a 128pt icon at 13pt text.
let labelOffset: CGFloat = 84
let plate = CGSize(width: 112, height: 22)

func ensoPoints() -> [CGPoint] {
    let svg = try! String(contentsOfFile: svgPath, encoding: .utf8)
    // The stroke is the path drawn with `stroke=`; the others are the tile.
    let path = svg.components(separatedBy: "<path ").first { $0.contains("stroke=\"#EDEAE2\"") }!
    let d = path.components(separatedBy: "d=\"")[1].components(separatedBy: "\"")[0]
    return d.split(whereSeparator: { $0 == "M" || $0 == "L" }).compactMap { pair in
        let xy = pair.trimmingCharacters(in: .whitespaces).split(separator: ",")
        guard xy.count == 2, let x = Double(xy[0]), let y = Double(xy[1]) else { return nil }
        return CGPoint(x: x, y: y)
    }
}

func font(size: CGFloat, weight: CGFloat) -> CTFont {
    let data = try! Data(contentsOf: URL(fileURLWithPath: fontPath)) as CFData
    let descriptors = CTFontManagerCreateFontDescriptorsFromData(data) as! [CTFontDescriptor]
    let base = CTFontCreateWithFontDescriptor(descriptors[0], size, nil)
    let wght = 0x7767_6874 // 'wght'
    return CTFontCreateCopyWithAttributes(
        base, size, nil,
        CTFontDescriptorCreateWithAttributes([kCTFontVariationAttribute: [wght: weight]] as CFDictionary))
}

func draw(_ text: String, font: CTFont, color: CGColor, tracking: CGFloat = 0, at point: CGPoint, centred: Bool = false, in ctx: CGContext) {
    let attributed = NSAttributedString(string: text, attributes: [
        .font: font, .foregroundColor: NSColor(cgColor: color)!, .kern: tracking,
    ])
    let line = CTLineCreateWithAttributedString(attributed)
    let width = CTLineGetTypographicBounds(line, nil, nil, nil)
    ctx.saveGState()
    // Text is laid out bottom-up; the canvas is flipped to read top-down.
    ctx.textMatrix = CGAffineTransform(scaleX: 1, y: -1)
    ctx.textPosition = CGPoint(x: centred ? point.x - width / 2 : point.x, y: point.y)
    CTLineDraw(line, ctx)
    ctx.restoreGState()
}

/// A stroke that swells and tapers like a brush, as a filled outline.
func brush(along curve: (CGFloat) -> CGPoint, width: (CGFloat) -> CGFloat, steps: Int = 160) -> CGPath {
    var left: [CGPoint] = [], right: [CGPoint] = []
    for i in 0...steps {
        let t = CGFloat(i) / CGFloat(steps)
        let p = curve(t)
        let q = curve(min(t + 0.001, 1)), r = curve(max(t - 0.001, 0))
        var n = CGPoint(x: -(q.y - r.y), y: q.x - r.x)
        let len = max(hypot(n.x, n.y), .ulpOfOne)
        n = CGPoint(x: n.x / len, y: n.y / len)
        let w = width(t) / 2
        left.append(CGPoint(x: p.x + n.x * w, y: p.y + n.y * w))
        right.append(CGPoint(x: p.x - n.x * w, y: p.y - n.y * w))
    }
    let path = CGMutablePath()
    path.addLines(between: left + right.reversed())
    path.closeSubpath()
    return path
}

func render(scale: CGFloat) -> CGImage {
    let ctx = CGContext(
        data: nil, width: Int(size.width * scale), height: Int(size.height * scale),
        bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpace(name: CGColorSpace.sRGB)!,
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    ctx.translateBy(x: 0, y: size.height * scale)
    ctx.scaleBy(x: scale, y: -scale)

    ctx.setFillColor(ground)
    ctx.fill(CGRect(origin: .zero, size: size))

    // Warmth behind the app, as if the record were already playing.
    let glow = CGGradient(colorsSpace: nil, colors: [brand.copy(alpha: 0.2)!, brand.copy(alpha: 0)!] as CFArray, locations: [0, 1])!
    ctx.drawRadialGradient(glow, startCenter: app, startRadius: 0, endCenter: app, endRadius: 190, options: [])

    // The ensō closes around the destination: the icon's stroke, redrawn as a target.
    let enso = ensoPoints()
    let ensoScale: CGFloat = 0.62
    let ensoPath = CGMutablePath()
    ensoPath.addLines(between: enso.map {
        CGPoint(x: applications.x + ($0.x - 256) * ensoScale, y: applications.y + ($0.y - 253) * ensoScale)
    })
    // In a layer, so the stroke's overlapping segments do not stack their alpha.
    ctx.setAlpha(0.07)
    ctx.beginTransparencyLayer(auxiliaryInfo: nil)
    ctx.addPath(ensoPath)
    ctx.setLineCap(.round)
    ctx.setLineJoin(.round)
    ctx.setLineWidth(40 * ensoScale)
    ctx.setStrokeColor(ink)
    ctx.strokePath()
    ctx.endTransparencyLayer()
    ctx.setAlpha(1)

    // One brushstroke from the app to the folder, arcing over the gap.
    let from = CGPoint(x: app.x + 88, y: app.y - 22)
    let to = CGPoint(x: applications.x - 104, y: applications.y - 22)
    let control = CGPoint(x: (from.x + to.x) / 2, y: from.y - 58)
    let arc = { (t: CGFloat) -> CGPoint in
        let u = 1 - t
        return CGPoint(
            x: u * u * from.x + 2 * u * t * control.x + t * t * to.x,
            y: u * u * from.y + 2 * u * t * control.y + t * t * to.y)
    }
    ctx.setFillColor(ink.copy(alpha: 0.8)!)
    ctx.addPath(brush(along: arc, width: { t in 1 + 5.5 * pow(sin(.pi * pow(t, 0.8)), 0.7) + 1.5 * t }))
    ctx.fillPath()

    // The head: two flicks back from the tip, along the stroke's final direction.
    let tip = arc(1), before = arc(0.97)
    let heading = atan2(tip.y - before.y, tip.x - before.x)
    for side: CGFloat in [-1, 1] {
        let angle = heading + .pi + side * 0.62
        let end = CGPoint(x: tip.x + cos(angle) * 17, y: tip.y + sin(angle) * 17)
        ctx.addPath(brush(along: { t in CGPoint(x: tip.x + (end.x - tip.x) * t, y: tip.y + (end.y - tip.y) * t) },
                          width: { t in 3.4 * (1 - t) + 0.6 }, steps: 24))
        ctx.fillPath()
    }

    ctx.setFillColor(ink)
    for icon in [app, applications] {
        let rect = CGRect(x: icon.x - plate.width / 2, y: icon.y + labelOffset - plate.height / 2,
                          width: plate.width, height: plate.height)
        ctx.addPath(CGPath(roundedRect: rect, cornerWidth: plate.height / 2, cornerHeight: plate.height / 2, transform: nil))
        ctx.fillPath()
    }

    draw("kōan", font: font(size: 26, weight: 200), color: ink, tracking: 0.5, at: CGPoint(x: 30, y: 48), in: ctx)
    draw("drag into Applications to install", font: font(size: 11, weight: 400), color: muted, tracking: 0.3,
         at: CGPoint(x: size.width / 2, y: 356), centred: true, in: ctx)

    // Grain, so the gradient does not band. Seeded: the same every build.
    var seed: UInt64 = 0x6B6F_616E
    let data = ctx.data!.assumingMemoryBound(to: UInt8.self)
    for i in 0..<(ctx.bytesPerRow * ctx.height) where i % 4 == 0 {
        seed = seed &* 6_364_136_223_846_793_005 &+ 1_442_695_040_888_963_407
        let n = Int((seed >> 33) % 7) - 3
        for c in 0..<3 { data[i + c] = UInt8(clamping: Int(data[i + c]) + n) }
    }
    return ctx.makeImage()!
}

for (scale, name) in [(1.0, "background.png"), (2.0, "background@2x.png")] {
    let url = URL(fileURLWithPath: outDir).appendingPathComponent(name) as CFURL
    let dest = CGImageDestinationCreateWithURL(url, "public.png" as CFString, 1, nil)!
    // 144 dpi marks the 2x image as the same size in points; tiffutil pairs them by it.
    let dpi = 72 * scale
    CGImageDestinationAddImage(dest, render(scale: scale),
                               [kCGImagePropertyDPIWidth: dpi, kCGImagePropertyDPIHeight: dpi] as CFDictionary)
    CGImageDestinationFinalize(dest)
}
