import CoreGraphics
import Foundation
import ImageIO
@testable import Koan
import SwiftUI
import Testing
import UniformTypeIdentifiers

/// The record accent keeps the sleeve's hue: a red sleeve gives a red accent,
/// however little of the sleeve is red, and only a sleeve with no colour at
/// all gives mint.
struct AccentTests {
    /// sRGB red's hue in OKLCH.
    private let red = OKLCH.from(srgb: 0xFF0000).h

    @Test func redStrokesOnBlackGiveARedAccent() throws {
        let sleeve = try #require(Color.dominant(ofEncoded: image { context in
            context.setStrokeColor(CGColor(srgbRed: 0.85, green: 0.1, blue: 0.12, alpha: 1))
            context.setLineWidth(4)
            for x in stride(from: 40, to: 480, by: 90) {
                context.move(to: CGPoint(x: x, y: 60))
                context.addLine(to: CGPoint(x: x + 40, y: 450))
            }
            context.strokePath()
        }))
        let accent = KoanAccent.of(sleeve)
        #expect(accent != .mint)
        for scheme in [ColorScheme.dark, .light] {
            #expect(distance(hue(accent.shade(scheme)), red) < 15, "\(scheme)")
        }
    }

    /// A red leaning towards blue sits beside `bad`'s hue, and once was turned
    /// past it into magenta.
    @Test func aRedNearBadStaysRed() {
        let accent = KoanAccent.of(Color(hue: 0.98, saturation: 0.6, brightness: 0.8))
        for scheme in [ColorScheme.dark, .light] {
            #expect(distance(hue(accent.shade(scheme)), red) < 15, "\(scheme)")
        }
    }

    @Test func aGreySleeveGivesMint() {
        let sleeve = Color.dominant(ofEncoded: image { context in
            context.setFillColor(CGColor(gray: 0.5, alpha: 1))
            context.fill(CGRect(x: 100, y: 100, width: 300, height: 300))
        })
        #expect(sleeve == nil)
        #expect(KoanAccent.of(sleeve) == .mint)
    }

    /// A black 512-point sleeve with `draw` on it, as PNG bytes.
    private func image(_ draw: (CGContext) -> Void) -> Data {
        let context = CGContext(
            data: nil, width: 512, height: 512, bitsPerComponent: 8, bytesPerRow: 0,
            space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        )!
        context.setFillColor(CGColor(gray: 0, alpha: 1))
        context.fill(CGRect(x: 0, y: 0, width: 512, height: 512))
        draw(context)
        let data = NSMutableData()
        let destination = CGImageDestinationCreateWithData(data, UTType.png.identifier as CFString, 1, nil)!
        CGImageDestinationAddImage(destination, context.makeImage()!, nil)
        CGImageDestinationFinalize(destination)
        return data as Data
    }

    private func hue(_ shade: KoanAccent.Shade) -> Double {
        let byte = { (v: Double) in UInt32((v * 255).rounded()) }
        return OKLCH.from(srgb: byte(shade.red) << 16 | byte(shade.green) << 8 | byte(shade.blue)).h
    }

    private func distance(_ a: Double, _ b: Double) -> Double {
        abs((a - b + 540).truncatingRemainder(dividingBy: 360) - 180)
    }
}
