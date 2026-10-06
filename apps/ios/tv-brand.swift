import AppKit

// Renders the tvOS brand assets: layered icons (a ground and the ensō, which
// the system lifts apart with parallax) and the Top Shelf banners, from the
// launch screen's ensō.
//
//     swiftc -O apps/ios/tv-brand.swift -o target/tv-brand
//     target/tv-brand apps/macos/Resources/Assets.xcassets/LaunchEnso.imageset/LaunchEnso.svg OUT
//
// then copy OUT's images over those in `App Icon & Top Shelf Image.brandassets`.
let args = CommandLine.arguments
let svg = NSImage(contentsOfFile: args[1])!
let out = URL(fileURLWithPath: args[2])

func render(_ w: Int, _ h: Int, _ draw: (CGContext) -> Void) -> Data {
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: w, pixelsHigh: h, bitsPerSample: 8,
                               samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                               colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    draw(NSGraphicsContext.current!.cgContext)
    NSGraphicsContext.restoreGraphicsState()
    return rep.representation(using: .png, properties: [:])!
}

let ground = CGColor(srgbRed: 0x15/255, green: 0x15/255, blue: 0x1A/255, alpha: 1)
let mint = CGColor(srgbRed: 0x7D/255, green: 0xD3/255, blue: 0xA7/255, alpha: 0.16)
let clear = CGColor(srgbRed: 0x7D/255, green: 0xD3/255, blue: 0xA7/255, alpha: 0)

// The ground: koan's near-black, with the faint mint the room glows when
// nothing is playing, off-centre so the parallax has something to move over.
func back(_ w: Int, _ h: Int) -> Data {
    render(w, h) { c in
        c.setFillColor(ground)
        c.fill(CGRect(x: 0, y: 0, width: w, height: h))
        let g = CGGradient(colorsSpace: CGColorSpace(name: CGColorSpace.sRGB), colors: [mint, clear] as CFArray, locations: [0, 1])!
        let r = Double(max(w, h)) * 0.7
        c.drawRadialGradient(g, startCenter: CGPoint(x: Double(w) * 0.3, y: Double(h) * 0.75), startRadius: 0,
                             endCenter: CGPoint(x: Double(w) * 0.3, y: Double(h) * 0.75), endRadius: r, options: [])
    }
}

// The ensō alone, on nothing, at `scale` of the height.
func front(_ w: Int, _ h: Int, scale: Double, x: Double = 0.5) -> Data {
    render(w, h) { _ in
        let side = Double(h) * scale
        svg.draw(in: NSRect(x: Double(w) * x - side / 2, y: (Double(h) - side) / 2, width: side, height: side))
    }
}

func banner(_ w: Int, _ h: Int) -> Data {
    render(w, h) { c in
        c.setFillColor(ground)
        c.fill(CGRect(x: 0, y: 0, width: w, height: h))
        let g = CGGradient(colorsSpace: CGColorSpace(name: CGColorSpace.sRGB), colors: [mint, clear] as CFArray, locations: [0, 1])!
        c.drawRadialGradient(g, startCenter: CGPoint(x: Double(w) * 0.5, y: Double(h) * 0.5), startRadius: 0,
                             endCenter: CGPoint(x: Double(w) * 0.5, y: Double(h) * 0.5), endRadius: Double(h) * 1.1, options: [])
        let side = Double(h) * 0.62
        svg.draw(in: NSRect(x: (Double(w) - side) / 2, y: (Double(h) - side) / 2, width: side, height: side))
    }
}

func write(_ data: Data, _ path: String) {
    let url = out.appendingPathComponent(path)
    try! FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
    try! data.write(to: url)
}

for (name, w, h) in [("small", 400, 240), ("small@2x", 800, 480), ("large", 1280, 768)] {
    write(back(w, h), "back-\(name).png")
    write(front(w, h, scale: 0.72), "front-\(name).png")
}
write(banner(1920, 720), "shelf.png")
write(banner(3840, 1440), "shelf@2x.png")
write(banner(2320, 720), "shelf-wide.png")
write(banner(4640, 1440), "shelf-wide@2x.png")
