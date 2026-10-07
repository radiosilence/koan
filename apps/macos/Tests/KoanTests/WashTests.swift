import CoreGraphics
@testable import Koan
import QuartzCore
import Testing

/// A record with no art clears the wash rather than leaving the one before it on.
@MainActor
struct WashTests {
    private func sleeve() throws -> CGImage {
        let context = try #require(CGContext(
            data: nil, width: 4, height: 4, bitsPerComponent: 8, bytesPerRow: 16,
            space: CGColorSpace(name: CGColorSpace.sRGB)!,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        ))
        context.setFillColor(CGColor(srgbRed: 0.8, green: 0.1, blue: 0.1, alpha: 1))
        context.fill(CGRect(x: 0, y: 0, width: 4, height: 4))
        return try #require(context.makeImage())
    }

    @Test func aRecordWithoutArtFadesTheLastOneOut() throws {
        let wash = WashView(frame: CGRect(x: 0, y: 0, width: 100, height: 100))
        wash.install(try sleeve())
        wash.install(nil)
        #expect(wash.current.contents == nil)
        #expect(wash.previous.opacity == 0)
        let fade = try #require(wash.previous.animation(forKey: "dissolve") as? CABasicAnimation)
        #expect(fade.toValue as? Int == 0)
    }

    @Test func aSleeveAfterNoArtFadesIn() throws {
        let wash = WashView(frame: CGRect(x: 0, y: 0, width: 100, height: 100))
        wash.install(try sleeve())
        wash.install(nil)
        wash.install(try sleeve())
        #expect(wash.current.contents != nil)
        #expect(wash.current.animation(forKey: "dissolve") != nil)
    }
}
