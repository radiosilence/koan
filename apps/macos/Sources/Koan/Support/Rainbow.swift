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
    /// else. Written by `AppearanceModel` before the tint it changes reaches
    /// them, and only ever on the main thread.
    nonisolated(unsafe) static var drawn = false
    /// The accent's place around the flag, for the same views.
    nonisolated(unsafe) static var step = 0
    /// A Charli XCX track brought the rainbow out, and brat's lime joins the
    /// flag for it.
    nonisolated(unsafe) static var brat = false

    /// The flag, top stripe first.
    nonisolated static let flag: [UInt32] = [0xE40303, 0xFF8C00, 0xFFED00, 0x008026, 0x004CFF, 0x732982]
    /// Brat green.
    nonisolated static let lime: UInt32 = 0x8ACE00
    /// The colours in force: the flag, and lime after it for brat.
    nonisolated static var colours: [UInt32] { brat ? flag + [lime] : flag }
    /// How long the accent holds each place.
    static let period: Duration = .seconds(2)

    /// The stripes as a sleeve for the wash, which blurs, tones and drifts
    /// them as it would a record.
    static var wash: PlatformImage { brat ? bratWash : prideWash }
    private static let prideWash = stripes(flag)
    private static let bratWash = stripes(flag + [lime])

    private static func stripes(_ colours: [UInt32]) -> PlatformImage {
        let side = 84
        let band = side / colours.count
        let context = CGContext(
            data: nil, width: side, height: side, bitsPerComponent: 8, bytesPerRow: 0,
            space: CGColorSpace(name: CGColorSpace.sRGB)!,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        )!
        for (index, hex) in colours.enumerated() {
            context.setFillColor(cgColor(hex))
            // Core Graphics counts up from the bottom; the flag reads down.
            context.fill(CGRect(x: 0, y: side - (index + 1) * band, width: side, height: band))
        }
        let image = context.makeImage()!
        #if canImport(AppKit)
        return NSImage(cgImage: image, size: CGSize(width: side, height: side))
        #else
        return UIImage(cgImage: image)
        #endif
    }

    nonisolated static func cgColor(_ hex: UInt32) -> CGColor {
        CGColor(
            srgbRed: CGFloat((hex >> 16) & 0xFF) / 255,
            green: CGFloat((hex >> 8) & 0xFF) / 255,
            blue: CGFloat(hex & 0xFF) / 255,
            alpha: 1
        )
    }

    /// Whether a track is by Charli XCX, as its artist or album artist,
    /// whatever the case and punctuation.
    nonisolated static func isCharli(_ entry: QueueItem) -> Bool {
        [entry.artist, entry.albumArtist].contains { folded($0).contains("charlixcx") }
    }

    nonisolated private static func folded(_ name: String) -> String {
        String(name.lowercased().unicodeScalars.filter(CharacterSet.alphanumerics.contains).map(Character.init))
    }

    /// What the toast says as the rainbow comes on, one at random.
    static let sass = [
        "gay mode: ON 💅✨",
        "gay mode: ON 💅✨ — the queue is a runway",
        "gay mode: ON 💅✨ — bit-perfect and fabulous",
        "gay mode: ON 💅✨ — lossless, shameless",
        "gay mode: ON 💅✨ — no notes",
        "gay mode: ON 💅✨ — hydrate, darling",
        "gay mode: ON 💅✨ — the wash is serving",
    ]
}

extension KoanAccent {
    /// The accent at a place around the flag, tone-mapped into the same bands
    /// as a record's, so text and indicators keep their contrast. Starts at
    /// the flag's blue: red would read as something gone wrong. Two places per
    /// stripe: each stripe, and one between it and the next.
    static func rainbow(_ step: Int) -> KoanAccent {
        let accents = Rainbow.brat ? bratAccents : prideAccents
        let count = accents.count
        return accents[((step + 8) % count + count) % count]
    }

    /// The colours as a gradient of accents, for what may carry it: progress,
    /// the chosen tab, a favourite's heart.
    static var pride: Gradient {
        Gradient(colors: (0..<Rainbow.colours.count).map { rainbow($0 * 2 - 8).color })
    }

    private static let prideAccents = around(Rainbow.flag)
    private static let bratAccents = around(Rainbow.flag + [Rainbow.lime])

    private static func around(_ colours: [UInt32]) -> [KoanAccent] {
        let hues = colours.map { OKLCH.from(srgb: $0).h }
        return (0..<hues.count * 2).map { step in
            let from = hues[step / 2], to = hues[(step / 2 + 1) % hues.count]
            // Round the short way: violet back to red passes through magenta.
            let turn = (to - from + 540).truncatingRemainder(dividingBy: 360) - 180
            let hue = (from + turn * Double(step % 2) / 2 + 360).truncatingRemainder(dividingBy: 360)
            guard let dark = shade(hue: hue, chroma: chroma.upperBound, band: darkBand, bg: 0x1E1E1E, surface: 0x2A2A2A),
                  let light = shade(hue: hue, chroma: chroma.upperBound, band: lightBand, bg: 0xFFFFFF, surface: 0xF2F2F2)
            else { return .mint }
            return KoanAccent(dark: dark, light: light)
        }
    }
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

    /// Start over: a key that cannot be part of the code.
    mutating func reset() { recent.removeAll() }

    /// A key; true when it completes the code.
    mutating func press(_ key: Key) -> Bool {
        recent.append(key)
        if recent.count > code.count { recent.removeFirst() }
        guard recent == code else { return false }
        recent.removeAll()
        return true
    }
}

extension EnvironmentValues {
    /// Where the rainbow's accent is around the flag, for what sweeps with it.
    @Entry var koanRainbowStep = 0
}

extension View {
    /// The now-playing title in gay mode: the flag through the text, moving
    /// along with the accent's cycle — no clock of its own. Plain with the
    /// rainbow off or motion reduced.
    func rainbowShimmer() -> some View { modifier(RainbowShimmer()) }
}

private struct RainbowShimmer: ViewModifier {
    @Environment(\.koanRainbow) private var rainbow
    @Environment(\.koanRainbowStep) private var step
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func body(content: Content) -> some View {
        if rainbow && !reduceMotion {
            let stops = KoanAccent.pride.stops.map(\.color)
            let shift = ((step / 2) % stops.count + stops.count) % stops.count
            let turned = Array(stops[shift...] + stops[..<shift])
            content
                .foregroundStyle(.clear)
                .overlay {
                    LinearGradient(colors: turned + [turned[0]], startPoint: .leading, endPoint: .trailing)
                        .mask(content)
                }
        } else {
            content
        }
    }
}

/// Confetti and sparkles over the window when the rainbow comes out, by hand
/// or for a track. Nothing is here between bursts: the emitters are made for
/// one, stop emitting after a moment and are removed when the last piece has
/// fallen. None with Reduce Motion.
struct RainbowBurst: View {
    @Environment(AppearanceModel.self) private var appearance
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var showing: Int?

    var body: some View {
        ZStack {
            if let showing { Confetti().id(showing) }
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
        .onChange(of: appearance.rainbowBurst) { _, burst in
            if !reduceMotion { showing = burst }
        }
        .task(id: showing) {
            guard showing != nil else { return }
            try? await Task.sleep(for: .seconds(3.2))
            guard !Task.isCancelled else { return }
            showing = nil
        }
    }
}

private struct Confetti: PlatformViewRepresentable {
    typealias PlatformViewType = ConfettiView
    func makeView(context: Context) -> ConfettiView { ConfettiView(frame: .zero) }
    func updateView(_ view: ConfettiView, context: Context) {}
}

/// The celebration: confetti, glitter and emoji raining from the top edge,
/// sparkles winking across the whole window. Capped: at most a few hundred
/// particles alive at the peak, all of them the render server's.
final class ConfettiView: LayerView {
    private let rain = CAEmitterLayer()
    private let sparkles = CAEmitterLayer()
    private var started = false

    nonisolated static let emoji = ["🏳️‍🌈", "💖", "✨", "🦄", "💅", "🪩"]

    override init(frame: CGRect) {
        super.init(frame: frame)
        // Down is down on both platforms.
        #if canImport(AppKit)
        hostLayer.isGeometryFlipped = true
        #endif
        let colours = Rainbow.colours
        let paper = Self.piece(width: 9, height: 4, diamond: false)
        let glitter = Self.piece(width: 3, height: 3, diamond: false)
        let diamond = Self.piece(width: 7, height: 7, diamond: true)
        rain.emitterShape = .line
        rain.emitterCells =
            colours.map { hex in
                Self.falling(paper, colour: hex, rate: 24, velocity: 280, spin: 8)
            }
            + colours.map { hex in
                let cell = Self.falling(glitter, colour: hex, rate: 30, velocity: 200, spin: 0)
                cell.alphaSpeed = -0.4
                return cell
            }
            + Self.emoji.map { face in
                let cell = Self.falling(Self.glyph(face), colour: nil, rate: 3, velocity: 160, spin: 1)
                cell.scale = 0.5
                cell.scaleRange = 0.15
                return cell
            }
        sparkles.emitterShape = .rectangle
        sparkles.emitterMode = .surface
        sparkles.emitterCells = colours.map { hex in
            let cell = CAEmitterCell()
            cell.contents = diamond
            cell.color = Rainbow.cgColor(hex)
            cell.birthRate = 7
            cell.lifetime = 1.2
            cell.lifetimeRange = 0.4
            cell.scale = 1.3
            cell.scaleSpeed = -1
            cell.alphaSpeed = -0.7
            cell.spin = 2
            return cell
        }
        for emitter in [rain, sparkles] { hostLayer.addSublayer(emitter) }
    }

    private static func falling(_ image: CGImage?, colour: UInt32?, rate: Float, velocity: CGFloat, spin: CGFloat) -> CAEmitterCell {
        let cell = CAEmitterCell()
        cell.contents = image
        if let colour { cell.color = Rainbow.cgColor(colour) }
        cell.birthRate = rate
        cell.lifetime = 3.2
        cell.velocity = velocity
        cell.velocityRange = velocity / 2
        cell.emissionLongitude = .pi / 2
        cell.emissionRange = .pi / 4
        cell.yAcceleration = 260
        cell.spin = spin
        cell.spinRange = spin * 2
        cell.scaleRange = 0.4
        return cell
    }

    override func layoutLayers() {
        let size = bounds.size
        guard size.width > 0, !started else { return }
        started = true
        rain.emitterPosition = CGPoint(x: size.width / 2, y: -20)
        rain.emitterSize = CGSize(width: size.width, height: 1)
        sparkles.emitterPosition = CGPoint(x: size.width / 2, y: size.height / 2)
        sparkles.emitterSize = size
        for emitter in [rain, sparkles] {
            emitter.frame = bounds
            emitter.beginTime = CACurrentMediaTime()
            emitter.birthRate = 1
        }
        // A burst, not a shower: emitting stops here and what is in the air
        // falls and fades by itself.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { [weak self] in
            self?.rain.birthRate = 0
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.8) { [weak self] in
            self?.sparkles.birthRate = 0
        }
    }

    /// A white piece the cell's colour tints: a strip of paper, or a diamond.
    nonisolated static func piece(width: Int, height: Int, diamond: Bool) -> CGImage? {
        let scale = 2
        guard let context = CGContext(
            data: nil, width: width * scale, height: height * scale, bitsPerComponent: 8, bytesPerRow: 0,
            space: CGColorSpace(name: CGColorSpace.sRGB)!,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        ) else { return nil }
        context.scaleBy(x: CGFloat(scale), y: CGFloat(scale))
        context.setFillColor(CGColor(srgbRed: 1, green: 1, blue: 1, alpha: 1))
        let w = CGFloat(width), h = CGFloat(height)
        if diamond {
            context.move(to: CGPoint(x: w / 2, y: 0))
            context.addLine(to: CGPoint(x: w, y: h / 2))
            context.addLine(to: CGPoint(x: w / 2, y: h))
            context.addLine(to: CGPoint(x: 0, y: h / 2))
            context.closePath()
            context.fillPath()
        } else {
            context.fill(CGRect(x: 0, y: 0, width: w, height: h))
        }
        return context.makeImage()
    }

    /// An emoji as a bitmap, drawn by Core Text in the colour font.
    nonisolated static func glyph(_ text: String, size: CGFloat = 56) -> CGImage? {
        let side = Int(size * 1.25)
        guard let context = CGContext(
            data: nil, width: side, height: side, bitsPerComponent: 8, bytesPerRow: 0,
            space: CGColorSpace(name: CGColorSpace.sRGB)!,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        ) else { return nil }
        let font = CTFontCreateWithName("AppleColorEmoji" as CFString, size, nil)
        let line = CTLineCreateWithAttributedString(
            NSAttributedString(string: text, attributes: [.init(kCTFontAttributeName as String): font])
        )
        let width = CTLineGetTypographicBounds(line, nil, nil, nil)
        context.textPosition = CGPoint(x: (CGFloat(side) - width) / 2, y: size * 0.25)
        CTLineDraw(line, context)
        return context.makeImage()
    }
}

/// A mirror ball in the window's corner while the rainbow is on: it sways on
/// its string and throws flecks of coloured light across the window. Both are
/// Core Animation's, committed once; nothing here wakes the main thread.
struct MirrorBall: View {
    /// Whether the app is in front. Behind, the flecks stop.
    let active: Bool
    @Environment(\.koanRainbow) private var rainbow
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        if rainbow && !reduceMotion {
            MirrorBallLayers(active: active)
                .allowsHitTesting(false)
                .accessibilityHidden(true)
                .transition(.opacity)
        }
    }
}

private struct MirrorBallLayers: PlatformViewRepresentable {
    let active: Bool
    typealias PlatformViewType = MirrorBallView
    func makeView(context: Context) -> MirrorBallView { MirrorBallView(frame: .zero) }
    func updateView(_ view: MirrorBallView, context: Context) {
        view.flecks.birthRate = active ? 1 : 0
    }
}

final class MirrorBallView: LayerView {
    private let ball = CALayer()
    /// Emitting only while the app is in front (`MirrorBallLayers`).
    let flecks = CAEmitterLayer()

    override init(frame: CGRect) {
        super.init(frame: frame)
        #if canImport(AppKit)
        hostLayer.isGeometryFlipped = true
        #endif
        let fleck = ConfettiView.piece(width: 5, height: 5, diamond: true)
        flecks.emitterShape = .rectangle
        flecks.emitterMode = .surface
        // A dozen alight at once, the window over.
        flecks.emitterCells = Rainbow.colours.map { hex in
            let cell = CAEmitterCell()
            cell.contents = fleck
            cell.color = Rainbow.cgColor(hex)
            cell.birthRate = 1
            cell.lifetime = 1.6
            cell.scale = 0.4
            cell.scaleSpeed = 0.6
            cell.alphaSpeed = -0.6
            return cell
        }
        ball.contents = ConfettiView.glyph("🪩", size: 64)
        ball.contentsGravity = .resizeAspect
        // Hung from the top of the window.
        ball.anchorPoint = CGPoint(x: 0.5, y: -0.6)
        for layer in [flecks, ball] { hostLayer.addSublayer(layer) }
    }

    override func layoutLayers() {
        let size = bounds.size
        guard size.width > 0 else { return }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        flecks.frame = bounds
        flecks.emitterPosition = CGPoint(x: size.width / 2, y: size.height / 2)
        flecks.emitterSize = size
        let side: CGFloat = 52
        ball.bounds = CGRect(x: 0, y: 0, width: side, height: side)
        ball.position = CGPoint(x: size.width - side - 24, y: 0)
        CATransaction.commit()
        guard ball.animation(forKey: "sway") == nil else { return }
        let sway = CABasicAnimation(keyPath: "transform.rotation.z")
        sway.fromValue = -0.12
        sway.toValue = 0.12
        sway.duration = 3.2
        sway.autoreverses = true
        sway.repeatCount = .infinity
        sway.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
        ball.add(sway, forKey: "sway")
    }
}

/// The disco over the wash: the colours as a conic gradient, its strength
/// following the music. Fed by `PlayingLevels` like the playing bars, so it
/// reads the analyser only while it is on screen — which is only while the
/// rainbow is on and motion allowed.
struct RainbowPulse: PlatformViewRepresentable {
    let levels: PlayingLevels

    typealias PlatformViewType = RainbowPulseView

    func makeView(context: Context) -> RainbowPulseView { RainbowPulseView(frame: .zero) }

    func updateView(_ view: RainbowPulseView, context: Context) {
        view.source = levels
        levels.attach(view)
    }

    static func dismantleView(_ view: RainbowPulseView, coordinator: ()) {
        view.source?.detach(view)
    }
}

final class RainbowPulseView: LayerView, LevelsListener {
    private let disco = CAGradientLayer()
    weak var source: PlayingLevels?

    override init(frame: CGRect) {
        super.init(frame: frame)
        disco.type = .conic
        disco.startPoint = CGPoint(x: 0.5, y: 0.5)
        disco.endPoint = CGPoint(x: 0.5, y: 0)
        let colours = Rainbow.colours.map(Rainbow.cgColor)
        disco.colors = colours + [colours[0]]
        disco.opacity = 0.12
        disco.actions = ["opacity": NSNull(), "bounds": NSNull(), "position": NSNull()]
        hostLayer.addSublayer(disco)
    }

    override func layoutLayers() {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        disco.frame = bounds
        CATransaction.commit()
    }

    /// A frame from the analyser: the low band, mostly, so it breathes with
    /// the beat. Gently: from a tint to a little more than one.
    func apply(_ bands: [Double]) {
        let level = bands.isEmpty ? 0 : (bands[0] * 0.6 + bands.dropFirst().reduce(0, +) * 0.2)
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        disco.opacity = Float(0.12 + min(max(level, 0), 1) * 0.22)
        CATransaction.commit()
    }
}
