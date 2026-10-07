#if canImport(AppKit)
import AppKit
#else
import UIKit
#endif
import KoanFFI
import SwiftUI

/// Gay mode: the accent, the wash, progress, selected tabs and the playing
/// bars in the pride flag's hues. Hidden on purpose — no Settings row, no docs.
/// It is switched by a gesture (the Konami code in the Mac's main window, seven
/// taps on the version line in iOS Settings, ↑↑↓↓←→←→ on a television's
/// Settings page), and switches itself on for some Charli XCX tracks.
///
/// Off, it costs nothing: the only timer is the accent's cycle, and that runs
/// only while the rainbow is drawn and motion is allowed.
@MainActor
enum Rainbow {
    /// Whether the rainbow is drawn, for layer views made outside SwiftUI —
    /// the table rows' playing bars — which are handed a tint and nothing
    /// else. Written by `AppearanceModel` before the tint it changes reaches them.
    static var drawn = false
    /// The accent's place around the flag, for the same views.
    static var step = 0

    /// The flag, top stripe first.
    nonisolated static let flag: [UInt32] = [0xE40303, 0xFF8C00, 0xFFED00, 0x008026, 0x004CFF, 0x732982]
    /// Places around the flag the accent stops at: each stripe, and one
    /// between it and the next.
    nonisolated static let steps = 12
    /// How long the accent holds each place.
    static let period: Duration = .seconds(2)

    /// The flag's stripes as a sleeve for the wash, which blurs, tones and
    /// drifts it as it would a record.
    static let wash: PlatformImage = {
        let side = 60
        let band = side / flag.count
        let context = CGContext(
            data: nil, width: side, height: side, bitsPerComponent: 8, bytesPerRow: 0,
            space: CGColorSpace(name: CGColorSpace.sRGB)!,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        )!
        for (index, hex) in flag.enumerated() {
            context.setFillColor(CGColor(
                srgbRed: CGFloat((hex >> 16) & 0xFF) / 255,
                green: CGFloat((hex >> 8) & 0xFF) / 255,
                blue: CGFloat(hex & 0xFF) / 255,
                alpha: 1
            ))
            // Core Graphics counts up from the bottom; the flag reads down.
            context.fill(CGRect(x: 0, y: side - (index + 1) * band, width: side, height: band))
        }
        let image = context.makeImage()!
        #if canImport(AppKit)
        return NSImage(cgImage: image, size: CGSize(width: side, height: side))
        #else
        return UIImage(cgImage: image)
        #endif
    }()

    /// Whether a track is by Charli XCX, as its artist or album artist,
    /// whatever the case and punctuation.
    nonisolated static func isCharli(_ entry: QueueItem) -> Bool {
        [entry.artist, entry.albumArtist].contains { folded($0).contains("charlixcx") }
    }

    nonisolated private static func folded(_ name: String) -> String {
        String(name.lowercased().unicodeScalars.filter(CharacterSet.alphanumerics.contains).map(Character.init))
    }
}

extension KoanAccent {
    /// The accent at a place around the flag, tone-mapped into the same bands
    /// as a record's, so text and indicators keep their contrast. Starts at
    /// the flag's blue: red would read as something gone wrong.
    static func rainbow(_ step: Int) -> KoanAccent {
        let count = rainbowAccents.count
        return rainbowAccents[((step + 8) % count + count) % count]
    }

    /// The flag as a gradient of accents, for the lines that may carry it:
    /// progress and the chosen tab.
    static var pride: Gradient {
        Gradient(colors: (0..<Rainbow.flag.count).map { rainbow($0 * 2 - 8).color })
    }

    private static let rainbowAccents: [KoanAccent] = {
        let hues = Rainbow.flag.map { OKLCH.from(srgb: $0).h }
        let perStripe = Rainbow.steps / hues.count
        return (0..<Rainbow.steps).map { step in
            let from = hues[step / perStripe], to = hues[(step / perStripe + 1) % hues.count]
            // Round the short way: violet back to red passes through magenta.
            let turn = (to - from + 540).truncatingRemainder(dividingBy: 360) - 180
            let hue = (from + turn * Double(step % perStripe) / Double(perStripe) + 360)
                .truncatingRemainder(dividingBy: 360)
            guard let dark = shade(hue: hue, chroma: chroma.upperBound, band: darkBand, bg: 0x1E1E1E, surface: 0x2A2A2A),
                  let light = shade(hue: hue, chroma: chroma.upperBound, band: lightBand, bg: 0xFFFFFF, surface: 0xF2F2F2)
            else { return .mint }
            return KoanAccent(dark: dark, light: light)
        }
    }()
}

extension EnvironmentValues {
    /// Whether the rainbow is drawn. Set by the room beside the accent.
    @Entry var koanRainbow = false
}

extension KoanTheme {
    /// What an underline or rule marking the chosen place is drawn in: the
    /// accent, or with the rainbow on, the flag.
    nonisolated static func marker(rainbow: Bool, vertical: Bool = false) -> AnyShapeStyle {
        rainbow
            ? AnyShapeStyle(LinearGradient(
                gradient: KoanAccent.pride,
                startPoint: vertical ? .top : .leading,
                endPoint: vertical ? .bottom : .trailing
            ))
            : AnyShapeStyle(.tint)
    }
}

/// A brief word when the rainbow is switched, over whichever window or screen
/// switched it.
struct RainbowToast: View {
    @Environment(AppearanceModel.self) private var appearance

    var body: some View {
        if let toast = appearance.rainbowToast {
            Text(toast)
                .koanText(.control, .ink)
                .padding(.horizontal, 16)
                .padding(.vertical, 10)
                .background(KoanTheme.ground(.regularMaterial), in: Rectangle())
                .overlay { Rectangle().strokeBorder(KoanTheme.marker(rainbow: true), lineWidth: 2) }
                .padding(.bottom, 96)
                .transition(.opacity)
                .allowsHitTesting(false)
                .task(id: toast) {
                    try? await Task.sleep(for: .seconds(1.6))
                    guard !Task.isCancelled else { return }
                    appearance.rainbowToast = nil
                }
        }
    }
}

/// Decides, as each track starts, whether a Charli XCX track brings the
/// rainbow out for itself: one in four do. Its own view, so the now-playing
/// slice it reads re-runs nothing else.
struct RainbowForTrack: View {
    @Environment(PlayerModel.self) private var player
    @Environment(AppearanceModel.self) private var appearance

    var body: some View {
        Color.clear
            .allowsHitTesting(false)
            .accessibilityHidden(true)
            .onChange(of: player.currentItemId, initial: true) { _, _ in
                appearance.trackStarted(player.currentEntry)
            }
    }
}

/// The last few keys pressed, against a code.
struct SecretCode<Key: Equatable> {
    let code: [Key]
    private var recent: [Key] = []

    init(_ code: [Key]) { self.code = code }

    /// A key; true when it completes the code.
    mutating func press(_ key: Key) -> Bool {
        recent.append(key)
        if recent.count > code.count { recent.removeFirst() }
        guard recent == code else { return false }
        recent.removeAll()
        return true
    }
}
