#if canImport(AppKit)
import AppKit
#else
import UIKit
#endif
import CoreText
import os
import KoanFFI
import SwiftUI

/// The kōan theme: the site's look carried into the app, when
/// `appearance.theme = "koan"`. The tokens and components are set down in
/// `docs/design/koan-theme.md`; this is their Swift form.
///
/// Views name roles — `.koanText(.title)`, `.koanSurface()`, `.koanButton(.primary)`,
/// `KoanLabel` — and never a colour or a font. Each role draws the theme when it
/// is on and the platform's nearest equivalent when it is off, so a converted
/// view carries no styling of its own in either.
///
/// Read once, as the app opens, and never changed while it runs: a theme
/// switched under a window would leave every view drawn before the switch in
/// the old one.
@MainActor
enum KoanTheme {
    /// The kōan theme, rather than the platform's look. Written once, as the
    /// app opens and before any view is drawn, and only read after: so read
    /// from anywhere, layer code off the main actor included.
    nonisolated(unsafe) private(set) static var isOn = false

    static func apply(_ appearance: Appearance) {
        isOn = appearance.koan
        guard isOn else { return }
        registerFace()
        #if os(iOS)
        // Navigation titles are UIKit's, drawn from its appearance proxies.
        let bar = UINavigationBar.appearance()
        bar.largeTitleTextAttributes = [.font: UIFont.koan(.display), .foregroundColor: UIColor.koanStrong]
        bar.titleTextAttributes = [.font: UIFont.koan(.control), .foregroundColor: UIColor.koanStrong]
        #endif
    }

    /// Geist Mono, bundled beside the app: the site's own file, which Core
    /// Text reads as it is, variable axes and all. Without it (a `swift run`
    /// build has no bundle) the type falls back to the system's monospace.
    private static func registerFace() {
        guard let url = Bundle.main.url(forResource: "geist-mono", withExtension: "woff2") else {
            return
        }
        CTFontManagerRegisterFontsForURL(url as CFURL, .process, nil)
    }

    /// A title the app writes, lowercased in the theme, for the few places
    /// that take a bare string — navigation titles, AppKit labels. Everywhere
    /// else the case changes only on screen (`.koanCase()`, and the theme's
    /// button and label styles), so accessibility labels, and the UI tests
    /// that find things by them, keep the words as written.
    nonisolated static func label(_ text: String) -> String {
        isOn ? text.lowercased() : text
    }

    /// How strongly the wash shows through where the design leaves the ground
    /// bare: the share of the toned sleeve mixed over `bg`.
    static var wash: Double {
        #if os(tvOS)
        0.5
        #else
        0.6
        #endif
    }

    /// Spacing steps. Page margins are `xl` on a phone, `xxl` on the Mac.
    enum Space {
        static let xs: CGFloat = 4
        static let s: CGFloat = 8
        static let m: CGFloat = 12
        static let l: CGFloat = 16
        static let xl: CGFloat = 22
        static let xxl: CGFloat = 32
        static var page: CGFloat {
            #if os(macOS)
            xxl
            #else
            xl
            #endif
        }
    }

    /// A corner as whichever look is on: square in the theme, the given
    /// radius otherwise.
    nonisolated static func radius(_ system: CGFloat) -> CGFloat { isOn ? 0 : system }

    /// A shadow's opacity as whichever look is on: none in the theme.
    nonisolated static func shadow(_ system: Float) -> Float { isOn ? 0 : system }

    /// Motion: fast and direct. A quick-out curve — a snappy start and a
    /// decisive stop, no tail, no overshoot, no delay. States (pressed, hover,
    /// selection, toggles) take `fast`; a marker moving between places, a
    /// tab's underline, takes `normal`; the accent arriving with a record takes
    /// `settle`, and never draws the eye. With Reduce Motion, all of it is
    /// instant: views take these through `.koanAnimation`, which knows.
    enum Motion {
        private static func quickOut(_ seconds: Double) -> Animation {
            .timingCurve(0.2, 0.9, 0.3, 1, duration: seconds)
        }

        static let fast = quickOut(0.08)
        static let normal = quickOut(0.12)
        static let settle = quickOut(0.25)
    }

    /// One rule's width: a point, not a pixel, which at `rule`'s contrast is too
    /// faint on a 2× display.
    static let hairline: CGFloat = 1
}

// MARK: - Icons

extension EnvironmentValues {
    /// Whether icons are drawn beside labels in the kōan theme ("Show icons"
    /// in Settings → Appearance). The platform's look always draws them.
    @Entry var koanIcons = true
}

/// What the app shows about how it is drawn, and the one place a change to it
/// is made: held by `AppState`, read by every window.
@MainActor
@Observable
final class AppearanceModel {
    private let engine: KoanEngine
    /// "Show icons".
    var showIcons: Bool {
        didSet { if showIcons != oldValue { engine.setThemeIcons(on: showIcons) } }
    }

    /// The theme chosen in Settings, which may not be the one drawn: it takes
    /// effect on the next launch (`KoanTheme.isOn` is the one drawn).
    var koan: Bool {
        didSet { if koan != oldValue { engine.setTheme(koan: koan) } }
    }

    /// "Colours from the record": the wash, and the accent from the sleeve.
    /// Off, no wash and koan's mint, in either theme. Takes effect at once.
    var recordColours: Bool {
        didSet { if recordColours != oldValue { engine.setRecordColours(on: recordColours) } }
    }

    init(engine: KoanEngine, appearance: Appearance) {
        self.engine = engine
        self.showIcons = appearance.icons
        self.koan = appearance.koan
        self.recordColours = appearance.recordColours
    }
}

/// One of the app's icons (`Icon.*`), drawn as the theme draws icons: a thin
/// monochrome line in the colour of the text beside it.
struct KoanIcon: View {
    let name: String

    init(_ name: String) { self.name = name }

    var body: some View {
        if KoanTheme.isOn {
            Image(systemName: name)
                .fontWeight(.light)
                .symbolRenderingMode(.monochrome)
        } else {
            Image(systemName: name)
        }
    }
}

/// A label the app writes, with its icon: what navigation rows, tabs, actions
/// and bar buttons are made of. In the platform's look, a `Label` as ever. In
/// the kōan theme, lowercase, with its icon drawn by `KoanIcon` or left out as
/// "Show icons" says. Every icon-or-not decision is made here and nowhere else.
struct KoanLabel: View {
    /// How much of the label shows when icons are on.
    enum Style {
        /// The title, with the icon before it.
        case full
        /// The icon alone, where space is a bar's (the transport's buttons);
        /// the title still names it to VoiceOver, and stands in for it when
        /// icons are off.
        case compact
    }

    let title: String
    let icon: String
    var style: Style = .full

    @Environment(\.koanIcons) private var icons

    init(_ title: String, icon: String, style: Style = .full) {
        self.title = title
        self.icon = icon
        self.style = style
    }

    var body: some View {
        if KoanTheme.isOn {
            Label {
                Text(title).koanCase()
            } icon: {
                KoanIcon(icon)
            }
            .labelStyle(KoanLabelStyle(icons: icons, style: style))
            .accessibilityLabel(title)
        } else if style == .compact {
            Label(title, systemImage: icon).labelStyle(.iconOnly)
        } else {
            Label(title, systemImage: icon)
        }
    }
}

private struct KoanLabelStyle: LabelStyle {
    let icons: Bool
    let style: KoanLabel.Style

    @ViewBuilder
    func makeBody(configuration: Configuration) -> some View {
        switch (icons, style) {
        case (false, _): Label(configuration).labelStyle(.titleOnly)
        case (true, .compact): Label(configuration).labelStyle(.iconOnly)
        case (true, .full): Label(configuration).labelStyle(.titleAndIcon)
        }
    }
}

// MARK: - Colour

extension Color {
    static let koanBg = Color.koan(dark: 0x1E1E1E, light: 0xFFFFFF)
    static let koanSurface = Color.koan(dark: 0x2A2A2A, light: 0xF2F2F2)
    static let koanRule = Color.koan(dark: 0x383838, light: 0xE0E0E0)
    static let koanHover = Color.koan(dark: 0x4D4D4D, light: 0xC4C4C4)
    static let koanInk = Color.koan(dark: 0xCCCCCC, light: 0x333333)
    static let koanStrong = Color.koan(dark: 0xFFFFFF, light: 0x111111)
    static let koanMuted = Color.koan(dark: 0x919191, light: 0x666666)
    static let koanBad = Color.koan(dark: 0xEF6B73, light: 0xC43F3F)

    fileprivate static func koan(dark: UInt32, light: UInt32) -> Color {
        #if canImport(AppKit)
        Color(nsColor: NSColor.koan(dark: dark, light: light))
        #else
        Color(uiColor: UIColor { $0.userInterfaceStyle == .dark ? .rgb(dark) : .rgb(light) })
        #endif
    }
}

#if canImport(AppKit)
extension NSColor {
    /// A token, following the appearance it is drawn in.
    static func koan(dark: UInt32, light: UInt32) -> NSColor {
        NSColor(name: nil) { appearance in
            appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? .rgb(dark) : .rgb(light)
        }
    }

    /// The tokens AppKit-drawn views (`KoanTable`, `AlbumCollection`,
    /// `MixedCollection`) read.
    static let koanBg = koan(dark: 0x1E1E1E, light: 0xFFFFFF)
    static let koanSurface = koan(dark: 0x2A2A2A, light: 0xF2F2F2)
    static let koanRule = koan(dark: 0x383838, light: 0xE0E0E0)
    static let koanHover = koan(dark: 0x4D4D4D, light: 0xC4C4C4)
    static let koanInk = koan(dark: 0xCCCCCC, light: 0x333333)
    static let koanStrong = koan(dark: 0xFFFFFF, light: 0x111111)
    static let koanMuted = koan(dark: 0x919191, light: 0x666666)

    /// The label colours AppKit-drawn rows use, as whichever look is on: the
    /// theme's tokens, or the system's label colours they stand in for.
    @MainActor static var koanLabel: NSColor { KoanTheme.isOn ? koanInk : .labelColor }
    @MainActor static var koanSecondaryLabel: NSColor { KoanTheme.isOn ? koanMuted : .secondaryLabelColor }
    @MainActor static var koanTertiaryLabel: NSColor { KoanTheme.isOn ? koanMuted : .tertiaryLabelColor }
    @MainActor static var koanQuaternaryLabel: NSColor { KoanTheme.isOn ? koanRule : .quaternaryLabelColor }
    static let koanBadToken = koan(dark: 0xEF6B73, light: 0xC43F3F)
    /// Errors, warnings and hearts: `bad` in the theme, the given system
    /// colour otherwise.
    @MainActor static func koanBad(_ system: NSColor) -> NSColor { KoanTheme.isOn ? koanBadToken : system }
    /// Hairlines between rows: `rule` in the theme.
    @MainActor static var koanSeparator: NSColor { KoanTheme.isOn ? koanRule : .separatorColor }
    /// A selected item's ground: `surface` in the theme.
    @MainActor static func koanSelection(_ system: NSColor) -> NSColor { KoanTheme.isOn ? koanSurface : system }

    fileprivate static func rgb(_ hex: UInt32) -> NSColor {
        NSColor(
            srgbRed: CGFloat((hex >> 16) & 0xFF) / 255,
            green: CGFloat((hex >> 8) & 0xFF) / 255,
            blue: CGFloat(hex & 0xFF) / 255,
            alpha: 1
        )
    }
}
#else
extension UIColor {
    /// A token, following the appearance it is drawn in.
    static func koan(dark: UInt32, light: UInt32) -> UIColor {
        UIColor { $0.userInterfaceStyle == .dark ? .rgb(dark) : .rgb(light) }
    }

    /// The tokens layer-drawn views read, as on the Mac.
    static let koanInk = koan(dark: 0xCCCCCC, light: 0x333333)
    static let koanStrong = koan(dark: 0xFFFFFF, light: 0x111111)
    static let koanRule = koan(dark: 0x383838, light: 0xE0E0E0)
    static let koanMuted = koan(dark: 0x919191, light: 0x666666)
    @MainActor static var koanQuaternaryLabel: UIColor { KoanTheme.isOn ? koanRule : .quaternaryLabel }

    fileprivate static func rgb(_ hex: UInt32) -> UIColor {
        UIColor(
            red: CGFloat((hex >> 16) & 0xFF) / 255,
            green: CGFloat((hex >> 8) & 0xFF) / 255,
            blue: CGFloat(hex & 0xFF) / 255,
            alpha: 1
        )
    }
}
#endif

/// The colours text and glyphs are drawn in. `accent` is the room's tint —
/// the record playing, or mint — and falls back to `ink` for text where the
/// record's colour cannot reach 4.5:1 (see `KoanAccent`).
enum KoanTone {
    /// `rule` is for strokes and fills — chart grids, dividers — never text.
    case ink, strong, muted, accent, bad, rule
}

// MARK: - Accent

/// The accent for a record, tone-mapped as the spec sets out: the sleeve's hue
/// kept, its lightness and chroma moved into a band per appearance in OKLCH,
/// clear of `bad`'s hue. Mint when there is no record or no usable hue. The
/// room's tint in both looks, not only the kōan theme's.
///
/// Built from the colour `Color.dominant` already works out for the room, so
/// there is one analysis of a sleeve, not two.
struct KoanAccent: Equatable, Sendable {
    struct Shade: Equatable, Sendable {
        let red, green, blue: Double
        /// Whether it reaches 4.5:1 on `bg` and `surface`. Where it does not,
        /// it is used for fills, indicators and rings only, and text stays ink.
        let readsAsText: Bool
    }

    let dark: Shade
    let light: Shade
    /// Built once, here: a dynamic colour made afresh on each read is a new
    /// value each time, and every view reading the tint would re-run with it.
    let color: Color

    static func == (a: KoanAccent, b: KoanAccent) -> Bool { a.dark == b.dark && a.light == b.light }

    static let mint = KoanAccent(
        dark: Shade(red: 0x7D / 255, green: 0xD3 / 255, blue: 0xA7 / 255, readsAsText: true),
        light: Shade(red: 0x1F / 255, green: 0x7A / 255, blue: 0x50 / 255, readsAsText: true)
    )

    /// Lightness bands, per appearance: never dark in dark mode, never pastel
    /// in light.
    static let darkBand = 0.70...0.85
    static let lightBand = 0.45...0.60
    /// Chroma floor and ceiling: a muddy sleeve gives a clean colour, a neon
    /// one does not flare.
    static let chroma = 0.10...0.19
    /// Under this, a sleeve has no hue worth carrying, and the accent is mint.
    static let noHue = 0.04
    /// Degrees of hue kept between the accent and `bad`, so a red record never
    /// reads as an error.
    static let badGap = 25.0

    func shade(_ scheme: ColorScheme) -> Shade { scheme == .dark ? dark : light }

    private static func color(dark: Shade, light: Shade) -> Color {
        #if canImport(AppKit)
        Color(nsColor: NSColor(name: nil) { appearance in
            let s = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? dark : light
            return NSColor(srgbRed: s.red, green: s.green, blue: s.blue, alpha: 1)
        })
        #else
        Color(uiColor: UIColor { traits in
            let s = traits.userInterfaceStyle == .dark ? dark : light
            return UIColor(red: s.red, green: s.green, blue: s.blue, alpha: 1)
        })
        #endif
    }

    /// The accent for a record's colour, as `Color.dominant` gives it; `nil`
    /// is no record, or none with a colour. Worked out once per colour: the
    /// room asks on every pass, and the same record must give the same value.
    static func of(_ record: Color?) -> KoanAccent {
        guard let record else { return .mint }
        return cache.withLock { known in
            if let hit = known[record] { return hit }
            if known.count >= 64 { known.removeAll() }
            let made = KoanAccent(record: record)
            known[record] = made
            return made
        }
    }

    private static let cache = OSAllocatedUnfairLock(initialState: [Color: KoanAccent]())

    private init(record: Color?) {
        guard let record else { self = .mint; return }
        let resolved = record.resolve(in: EnvironmentValues())
        let (_, c, h) = OKLCH.from(
            linear: (Double(resolved.linearRed), Double(resolved.linearGreen), Double(resolved.linearBlue))
        )
        guard c >= Self.noHue,
              let dark = Self.shade(hue: h, chroma: c, band: Self.darkBand, bad: 0xEF6B73, bg: 0x1E1E1E, surface: 0x2A2A2A),
              let light = Self.shade(hue: h, chroma: c, band: Self.lightBand, bad: 0xC43F3F, bg: 0xFFFFFF, surface: 0xF2F2F2)
        else { self = .mint; return }
        self.init(dark: dark, light: light)
    }

    private init(dark: Shade, light: Shade) {
        self.dark = dark
        self.light = light
        self.color = Self.color(dark: dark, light: light)
    }

    /// The most vivid lightness in the band that reads as text — the darkest in
    /// dark mode, the lightest in light — or failing that, the one that clears
    /// 3:1 as a fill.
    private static func shade(
        hue: Double, chroma: Double, band: ClosedRange<Double>,
        bad: UInt32, bg: UInt32, surface: UInt32
    ) -> Shade? {
        let badHue = OKLCH.from(srgb: bad).h
        var h = hue
        let d = (h - badHue + 540).truncatingRemainder(dividingBy: 360) - 180
        if abs(d) < badGap { h = (badHue + (d >= 0 ? badGap : -badGap) + 360).truncatingRemainder(dividingBy: 360) }
        let c = min(max(chroma, Self.chroma.lowerBound), Self.chroma.upperBound)
        let darkMode = band == darkBand
        let steps = (0...50).map { band.lowerBound + (band.upperBound - band.lowerBound) * Double($0) / 50 }
        let order = darkMode ? steps : steps.reversed()
        let bgL = OKLCH.luminance(srgb: bg), surfaceL = OKLCH.luminance(srgb: surface)
        func contrast(_ a: Double, _ b: Double) -> Double { (max(a, b) + 0.05) / (min(a, b) + 0.05) }
        for (minimum, text) in [(4.5, true), (3.0, false)] {
            for l in (text ? order : order.reversed()) {
                let rgb = OKLCH.toSRGB(l: l, c: c, h: h)
                let y = OKLCH.luminance(rgb)
                if contrast(y, bgL) >= minimum, contrast(y, surfaceL) >= minimum {
                    return Shade(red: rgb.0, green: rgb.1, blue: rgb.2, readsAsText: text)
                }
            }
        }
        return nil
    }
}

/// OKLab in polar form: what the accent is tone-mapped in, because its
/// lightness is perceptual and its hue holds still while lightness moves.
enum OKLCH {
    static func from(srgb hex: UInt32) -> (l: Double, c: Double, h: Double) {
        let r = Double((hex >> 16) & 0xFF) / 255, g = Double((hex >> 8) & 0xFF) / 255, b = Double(hex & 0xFF) / 255
        return from(linear: (linear(r), linear(g), linear(b)))
    }

    static func from(linear rgb: (Double, Double, Double)) -> (l: Double, c: Double, h: Double) {
        let (r, g, b) = rgb
        let l = cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b)
        let m = cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b)
        let s = cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b)
        let lightness = 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s
        let a = 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s
        let bb = 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s
        let hue = (atan2(bb, a) * 180 / .pi + 360).truncatingRemainder(dividingBy: 360)
        return (lightness, (a * a + bb * bb).squareRoot(), hue)
    }

    /// sRGB, with chroma given up until the colour is inside the gamut.
    static func toSRGB(l: Double, c: Double, h: Double) -> (Double, Double, Double) {
        var chroma = c
        while true {
            let lin = toLinear(l: l, c: chroma, h: h)
            let inside = [lin.0, lin.1, lin.2].allSatisfy { $0 >= -0.0001 && $0 <= 1.0001 }
            if inside || chroma <= 0 {
                return (gamma(lin.0), gamma(lin.1), gamma(lin.2))
            }
            chroma -= 0.002
        }
    }

    static func luminance(srgb hex: UInt32) -> Double {
        luminance((Double((hex >> 16) & 0xFF) / 255, Double((hex >> 8) & 0xFF) / 255, Double(hex & 0xFF) / 255))
    }

    static func luminance(_ rgb: (Double, Double, Double)) -> Double {
        0.2126 * linear(rgb.0) + 0.7152 * linear(rgb.1) + 0.0722 * linear(rgb.2)
    }

    private static func toLinear(l: Double, c: Double, h: Double) -> (Double, Double, Double) {
        let a = c * cos(h * .pi / 180), b = c * sin(h * .pi / 180)
        let l3 = pow(l + 0.3963377774 * a + 0.2158037573 * b, 3)
        let m3 = pow(l - 0.1055613458 * a - 0.0638541728 * b, 3)
        let s3 = pow(l - 0.0894841775 * a - 1.2914855480 * b, 3)
        return (
            4.0767416621 * l3 - 3.3077115913 * m3 + 0.2309699292 * s3,
            -1.2684380046 * l3 + 2.6097574011 * m3 - 0.3413193965 * s3,
            -0.0041960863 * l3 - 0.7034186147 * m3 + 1.7076147010 * s3
        )
    }

    private static func linear(_ v: Double) -> Double {
        v <= 0.04045 ? v / 12.92 : pow((v + 0.055) / 1.055, 2.4)
    }

    private static func gamma(_ v: Double) -> Double {
        let v = min(max(v, 0), 1)
        return v <= 0.0031308 ? 12.92 * v : 1.055 * pow(v, 1 / 2.4) - 0.055
    }
}

extension EnvironmentValues {
    /// The accent in force, for roles that must know whether it reads as text.
    /// Set beside `.tint` and `roomTint` by the room.
    @Entry var koanAccent = KoanAccent.mint
}

// MARK: - Type

/// The roles of the theme's type scale.
enum KoanType {
    case display, title, titleSmall, body, control, meta, fine

    /// Points, at the distance the device is read from: a television is read
    /// from across a room, so its scale is the same steps, larger.
    var size: CGFloat {
        #if os(tvOS)
        base * 1.8
        #else
        base
        #endif
    }

    private var base: CGFloat {
        switch self {
        case .display: 34
        case .title: 26
        case .titleSmall: 22
        case .body: 15
        case .control: 14
        case .meta: 13
        case .fine: 12
        }
    }

    var weight: Font.Weight {
        switch self {
        case .display: .ultraLight
        case .title, .titleSmall: .light
        default: .regular
        }
    }

    /// The system style each role scales with, so the platform's text size
    /// setting reaches it — and the font the role is in the platform's look.
    var scalesWith: Font.TextStyle {
        switch self {
        case .display: .largeTitle
        case .title: .title
        case .titleSmall: .title2
        case .body: .body
        case .control: .callout
        case .meta: .subheadline
        case .fine: .footnote
        }
    }
}

extension Font {
    /// A role of the theme's type scale, in Geist Mono.
    static func koan(_ role: KoanType) -> Font {
        .custom("Geist Mono", size: role.size, relativeTo: role.scalesWith).weight(role.weight)
    }

    /// A role as whichever look is on: Geist Mono in the theme, the role's
    /// text style otherwise. For views that take a font rather than a modifier.
    @MainActor
    static func role(_ role: KoanType) -> Font {
        KoanTheme.isOn ? .koan(role) : .system(role.scalesWith)
    }

    /// A role in the theme, and exactly the given font in the platform's look.
    @MainActor
    static func role(_ role: KoanType, system: Font) -> Font {
        KoanTheme.isOn ? .koan(role) : system
    }
}

extension KoanTheme {
    /// A tone as a style, for glyphs and shapes: the token in the theme, the
    /// nearest semantic style otherwise. Text takes `.koanText`, which also
    /// keeps a record's accent off text that it cannot reach 4.5:1 as.
    /// A tone in the theme, and exactly the given style in the platform's look.
    nonisolated static func style(_ tone: KoanTone, system: some ShapeStyle) -> AnyShapeStyle {
        isOn ? style(tone) : AnyShapeStyle(system)
    }

    nonisolated static func style(_ tone: KoanTone) -> AnyShapeStyle {
        switch (isOn, tone) {
        case (true, .ink): AnyShapeStyle(Color.koanInk)
        case (true, .strong): AnyShapeStyle(Color.koanStrong)
        case (true, .muted): AnyShapeStyle(Color.koanMuted)
        case (true, .bad): AnyShapeStyle(Color.koanBad)
        case (true, .rule): AnyShapeStyle(Color.koanRule)
        case (false, .rule): AnyShapeStyle(.quaternary)
        case (_, .accent): AnyShapeStyle(.tint)
        case (false, .ink), (false, .strong): AnyShapeStyle(.primary)
        case (false, .muted): AnyShapeStyle(.secondary)
        case (false, .bad): AnyShapeStyle(.red)
        }
    }
}

#if os(iOS)
extension UIFont {
    /// A role of the theme's type scale, for UIKit's own drawing (navigation
    /// titles), scaled with Dynamic Type as the role's text style is.
    static func koan(_ role: KoanType) -> UIFont {
        let weight: UIFont.Weight = switch role.weight {
        case .ultraLight: .ultraLight
        case .light: .light
        default: .regular
        }
        let base = UIFont(name: "Geist Mono", size: role.size)
            ?? .monospacedSystemFont(ofSize: role.size, weight: weight)
        let face = UIFont(
            descriptor: base.fontDescriptor.addingAttributes([.traits: [UIFontDescriptor.TraitKey.weight: weight]]),
            size: role.size
        )
        let style: UIFont.TextStyle = switch role.scalesWith {
        case .largeTitle: .largeTitle
        case .title: .title1
        case .title2: .title2
        case .callout: .callout
        case .subheadline: .subheadline
        case .footnote: .footnote
        default: .body
        }
        return UIFontMetrics(forTextStyle: style).scaledFont(for: face)
    }
}
#endif

#if canImport(AppKit)
extension NSFont {
    /// A role of the theme's type scale, for AppKit's own views. Falls back to
    /// the system monospace where the face is not registered.
    @MainActor
    static func koan(_ role: KoanType, weight: NSFont.Weight? = nil) -> NSFont {
        let wanted: NSFont.Weight = weight ?? {
            switch role.weight {
            case .ultraLight: .ultraLight
            case .light: .light
            default: .regular
            }
        }()
        let base = NSFont(name: "Geist Mono", size: role.size)
            ?? .monospacedSystemFont(ofSize: role.size, weight: wanted)
        let descriptor = base.fontDescriptor.addingAttributes([
            .traits: [NSFontDescriptor.TraitKey.weight: wanted],
        ])
        return NSFont(descriptor: descriptor, size: role.size) ?? base
    }

    /// A role as whichever look is on: Geist Mono in the theme, the given
    /// system font otherwise.
    @MainActor
    static func role(_ role: KoanType, system: @autoclosure () -> NSFont) -> NSFont {
        KoanTheme.isOn ? koan(role) : system()
    }
}
#endif

// MARK: - Roles

extension View {
    /// Text in a role of the type scale and a tone. In the platform's look, the
    /// role's text style and the nearest semantic colour.
    func koanText(_ role: KoanType, _ tone: KoanTone = .ink) -> some View {
        modifier(KoanTextRole(role: role, tone: tone))
    }

    /// The ground a region sits on: `bg`, or `surface` for a raised field.
    /// Nothing in the platform's look, which has its own.
    func koanSurface(_ surface: KoanSurface = .bg) -> some View {
        modifier(KoanSurfaceRole(surface: surface))
    }

    /// A rule along one edge, inset from the leading side. Nothing in the
    /// platform's look.
    func koanRule(_ edge: Edge = .bottom, inset: CGFloat = 0) -> some View {
        modifier(KoanRuleRole(edge: edge, inset: inset))
    }

    /// One of the theme's buttons. In the platform's look, the button as it
    /// was: whatever style it already had, or inherits.
    func koanButton(_ kind: KoanButtonKind) -> some View {
        modifier(KoanButtonRole(kind: kind, system: Optional<DefaultButtonStyle>.none))
    }

    /// One of the theme's buttons, and exactly `system` in the platform's look.
    func koanButton<S: PrimitiveButtonStyle>(_ kind: KoanButtonKind, system: S) -> some View {
        modifier(KoanButtonRole(kind: kind, system: system))
    }

    /// The theme's buttons of one kind for everything inside, leaving the
    /// platform's look as it was: for groups whose buttons already have the
    /// platform style they want.
    @ViewBuilder
    func koanButtons(_ kind: KoanButtonKind) -> some View {
        if KoanTheme.isOn {
            buttonStyle(KoanButtonStyle(kind: kind))
        } else {
            self
        }
    }

    /// A toggle as the theme draws it: a square box. The system's switch otherwise.
    func koanToggle() -> some View {
        modifier(KoanToggleRole())
    }

    /// A list row's ground: a rule below, `surface` when selected, the hover
    /// fill on the Mac. Inside a `List`, it also takes over the row's
    /// separator and background.
    func koanRow(selected: Bool = false) -> some View {
        modifier(KoanRowRole(selected: selected))
    }

    /// A navigation row, as the sidebar's: `body` in `muted`, or in the accent
    /// with a 2-point accent rule on its leading edge when it is where you are.
    /// The platform's sidebar row otherwise.
    func koanNavRow(selected: Bool) -> some View {
        modifier(KoanNavRowRole(selected: selected))
    }

    /// A sidebar's ground: flat `bg` in place of the system's material.
    func koanSidebar() -> some View {
        modifier(KoanSidebarRole())
    }

    /// Focus on tvOS, as the theme shows it: a ring in the accent. Elsewhere, and
    /// in the platform's look, the system's own.
    func koanFocus() -> some View {
        modifier(KoanFocusRole())
    }

    /// A small fact set apart, such as a format: `fine` in `ink` inside a
    /// square `rule` outline. A tinted capsule in the platform's look.
    func koanBadge() -> some View {
        modifier(KoanBadgeRole())
    }

    /// A chip that is a choice — a device, a preset, an artist: a `muted`
    /// outline, square, in the theme. A filled capsule in the platform's look.
    func koanChip() -> some View {
        modifier(KoanChipRole())
    }

    /// A bar along the window's foot, such as the transport: flat `bg` with a
    /// rule along its top, full width. In the platform's look, a floating slab
    /// of glass with the given corner radius, inset from the window's edges.
    func koanBar(radius: CGFloat, inset: CGFloat) -> some View {
        modifier(KoanBarRole(radius: radius, inset: inset))
    }

    /// A form as the theme lays one out: no cards, rows on the ground with
    /// rules between them. The platform's grouped form otherwise.
    func koanForm() -> some View {
        modifier(KoanFormRole())
    }

    /// A text field: the theme's type on a `surface` field, square, no bezel.
    /// The platform's field otherwise.
    func koanField() -> some View {
        modifier(KoanFieldRole())
    }

    /// A form section with no card behind its rows. Forms on the Mac draw a
    /// card per section, which `.koanForm()` cannot reach from outside it.
    func koanSection() -> some View {
        modifier(KoanSectionRole())
    }

    /// A pop-up picker, menu or stepper: the system control, in `ink` and the
    /// theme's type rather than the accent. Unchanged in the platform's look.
    func koanControl() -> some View {
        modifier(KoanControlRole())
    }

    /// A list as the theme lays one out: rows on the ground with rules between
    /// them, no inset cards. The platform's list otherwise.
    func koanList() -> some View {
        modifier(KoanListRole())
    }

    /// The window's toolbar, or a phone's navigation bar: flat `bg` in the
    /// theme. Otherwise hidden over the wash where the window's glass is
    /// affordable (`glass`), and the platform's own where it is not.
    @ViewBuilder
    func koanToolbar(glass: Bool) -> some View {
        #if os(tvOS)
        self
        #else
        modifier(KoanToolbarRole(glass: glass))
        #endif
    }

    /// A sheet's chrome: `bg` beneath, no material, the theme's type for
    /// everything that does not set its own.
    func koanSheet() -> some View {
        modifier(KoanSheetRole())
    }
}

enum KoanSurface { case bg, surface }

enum KoanButtonKind {
    /// The accent, outlined in it.
    case primary
    /// Ink, outlined in `muted`.
    case secondary
    /// `muted`, no outline; ink on hover. Bars' actions ("clear", "sleep").
    case text
    /// A glyph alone, with a 44-point hit area.
    case icon
    /// Play and pause: a glyph in a square `ink` outline.
    case iconOutlined
    /// A button that is a thing from the library — a record, a track, a
    /// person: its own content, type and case, with the theme's pressed fill
    /// and focus ring.
    case card

    /// Whether the theme sets the label's type. Glyphs keep the size the page
    /// gives them, and cards their own.
    fileprivate var setsType: Bool {
        switch self {
        case .primary, .secondary, .text: true
        case .icon, .iconOutlined, .card: false
        }
    }
}

private struct KoanTextRole: ViewModifier {
    let role: KoanType
    let tone: KoanTone
    @Environment(\.koanAccent) private var accent
    @Environment(\.colorScheme) private var scheme

    func body(content: Content) -> some View {
        if KoanTheme.isOn {
            content.font(.koan(role)).foregroundStyle(themed)
        } else {
            content.font(.system(role.scalesWith)).foregroundStyle(system)
        }
    }

    private var themed: AnyShapeStyle {
        switch tone {
        case .ink: AnyShapeStyle(Color.koanInk)
        case .strong: AnyShapeStyle(Color.koanStrong)
        case .muted: AnyShapeStyle(Color.koanMuted)
        case .bad: AnyShapeStyle(Color.koanBad)
        case .rule: AnyShapeStyle(Color.koanRule)
        case .accent: accent.shade(scheme).readsAsText ? AnyShapeStyle(.tint) : AnyShapeStyle(Color.koanInk)
        }
    }

    private var system: AnyShapeStyle {
        switch tone {
        case .ink, .strong: AnyShapeStyle(.primary)
        case .muted: AnyShapeStyle(.secondary)
        case .accent: accent.shade(scheme).readsAsText ? AnyShapeStyle(.tint) : AnyShapeStyle(.primary)
        case .bad: AnyShapeStyle(.red)
        case .rule: AnyShapeStyle(.quaternary)
        }
    }
}

private struct KoanSurfaceRole: ViewModifier {
    let surface: KoanSurface

    func body(content: Content) -> some View {
        if KoanTheme.isOn {
            content.background(surface == .bg ? Color.koanBg : Color.koanSurface)
        } else {
            content
        }
    }
}

private struct KoanRuleRole: ViewModifier {
    let edge: Edge
    let inset: CGFloat

    func body(content: Content) -> some View {
        if KoanTheme.isOn {
            content.overlay(alignment: alignment) {
                Rectangle()
                    .fill(Color.koanRule)
                    .frame(
                        width: edge == .leading || edge == .trailing ? KoanTheme.hairline : nil,
                        height: edge == .top || edge == .bottom ? KoanTheme.hairline : nil
                    )
                    .padding(.leading, edge == .top || edge == .bottom ? inset : 0)
                    .allowsHitTesting(false)
            }
        } else {
            content
        }
    }

    private var alignment: Alignment {
        switch edge {
        case .top: .top
        case .bottom: .bottom
        case .leading: .leading
        case .trailing: .trailing
        }
    }
}

private struct KoanButtonRole<S: PrimitiveButtonStyle>: ViewModifier {
    let kind: KoanButtonKind
    let system: S?

    func body(content: Content) -> some View {
        if KoanTheme.isOn {
            content.buttonStyle(KoanButtonStyle(kind: kind))
        } else if let system {
            content.buttonStyle(system)
        } else {
            content
        }
    }
}

/// The theme's buttons: square, outlined or bare, pressed to `hover`, with no
/// motion of their own.
struct KoanButtonStyle: ButtonStyle {
    let kind: KoanButtonKind

    func makeBody(configuration: Configuration) -> some View {
        KoanButtonBody(kind: kind, configuration: configuration)
    }
}

/// A theme button as drawn. A view of its own rather than the style's body:
/// focus is the button's, and only a view inside the button sees it in
/// `isFocused`; the style itself reads the environment the button sits in.
private struct KoanButtonBody: View {
    let kind: KoanButtonKind
    let configuration: ButtonStyleConfiguration
    @Environment(\.isEnabled) private var enabled
    @Environment(\.koanAccent) private var accent
    @Environment(\.colorScheme) private var scheme
    #if os(tvOS)
    @Environment(\.isFocused) private var focused
    #endif

    var body: some View {
        typed(configuration)
            .padding(padding)
            .frame(minWidth: hit, minHeight: hit)
            .background(configuration.isPressed ? Color.koanHover : .clear)
            .koanAnimation(KoanTheme.Motion.fast, value: configuration.isPressed)
            .overlay {
                if let outline {
                    Rectangle().strokeBorder(outline, lineWidth: KoanTheme.hairline)
                }
            }
            .contentShape(Rectangle())
            .opacity(enabled ? 1 : 0.4)
            .koanFocusRing(focusedNow)
    }

    /// Type and colour for the kinds that set them. Case is left to the label:
    /// a title the app writes is lowercased where it is written
    /// (`KoanTheme.label`, `KoanLabel`), and library text keeps its own.
    @ViewBuilder
    private func typed(_ configuration: ButtonStyleConfiguration) -> some View {
        if kind == .card {
            configuration.label
        } else if kind.setsType {
            configuration.label
                .font(.koan(.control))
                .textCase(.lowercase)
                .foregroundStyle(foreground(configuration))
        } else {
            configuration.label
                .foregroundStyle(foreground(configuration))
        }
    }

    private var focusedNow: Bool {
        #if os(tvOS)
        focused
        #else
        false
        #endif
    }

    private func foreground(_ configuration: ButtonStyleConfiguration) -> AnyShapeStyle {
        switch kind {
        case .primary:
            accent.shade(scheme).readsAsText ? AnyShapeStyle(.tint) : AnyShapeStyle(Color.koanInk)
        case .secondary, .icon, .iconOutlined, .card: AnyShapeStyle(Color.koanInk)
        case .text: AnyShapeStyle(configuration.isPressed ? Color.koanInk : Color.koanMuted)
        }
    }

    private var outline: AnyShapeStyle? {
        switch kind {
        case .primary: AnyShapeStyle(.tint)
        case .secondary: AnyShapeStyle(Color.koanMuted)
        case .iconOutlined: AnyShapeStyle(Color.koanInk)
        case .text, .icon, .card: nil
        }
    }

    private var padding: EdgeInsets {
        switch kind {
        case .primary, .secondary: EdgeInsets(top: 10, leading: 16, bottom: 10, trailing: 16)
        case .text: EdgeInsets(top: 4, leading: 0, bottom: 4, trailing: 0)
        case .icon, .card: EdgeInsets()
        case .iconOutlined: EdgeInsets(top: 7, leading: 7, bottom: 7, trailing: 7)
        }
    }

    private var hit: CGFloat? {
        switch kind {
        #if os(macOS)
        // A pointer, not a finger: the transport's glyphs sit two to a zone
        // the height of the seek bar and the controls together.
        case .icon, .iconOutlined: nil
        #else
        case .icon, .iconOutlined: 44
        #endif
        default: nil
        }
    }
}

private struct KoanToggleRole: ViewModifier {
    func body(content: Content) -> some View {
        if KoanTheme.isOn {
            #if os(tvOS)
            content
            #else
            content.toggleStyle(KoanToggleStyle())
            #endif
        } else {
            content
        }
    }
}

#if !os(tvOS)
#if os(macOS)
/// A form row's label in a column of its own, so the fields beside a run of
/// labels start at one edge.
struct KoanLabeledContentStyle: LabeledContentStyle {
    func makeBody(configuration: Configuration) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: KoanTheme.Space.m) {
            configuration.label
                .foregroundStyle(Color.koanMuted)
                .frame(width: 150, alignment: .leading)
            configuration.content
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}
#endif

/// A square box: a `muted` outline off, filled with the accent and checked in
/// `bg` on.
struct KoanToggleStyle: ToggleStyle {
    @Environment(\.isEnabled) private var enabled
    /// A finger's target on a phone; a pointer needs no more than the row.
    #if os(macOS)
    private static let hit: CGFloat = 24
    #else
    private static let hit: CGFloat = 44
    #endif

    func makeBody(configuration: Configuration) -> some View {
        Button {
            configuration.isOn.toggle()
        } label: {
            HStack(spacing: KoanTheme.Space.m) {
                ZStack {
                    if configuration.isOn {
                        Rectangle().fill(.tint)
                        Image(systemName: "checkmark")
                            .font(.system(size: 9, weight: .bold))
                            .foregroundStyle(Color.koanBg)
                    } else {
                        Rectangle().strokeBorder(Color.koanMuted, lineWidth: KoanTheme.hairline)
                    }
                }
                .frame(width: 14, height: 14)
                configuration.label
                    .font(.koan(.body))
                    .foregroundStyle(Color.koanInk)
                Spacer(minLength: 0)
            }
            .frame(minHeight: Self.hit)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .opacity(enabled ? 1 : 0.4)
        .koanAnimation(KoanTheme.Motion.fast, value: configuration.isOn)
        .accessibilityValue(configuration.isOn ? "On" : "Off")
        .accessibilityAddTraits(.isToggle)
    }
}
#endif

/// A segmented control as the theme draws it: options as text, `control` type,
/// 18 apart; the chosen one in `ink`, underlined in the accent. A view of its
/// own rather than a picker style: AppKit's and UIKit's segmented controls take
/// none. The system's segmented picker in the platform's look.
struct KoanSegmentedPicker<Value: Hashable>: View {
    let options: [(label: String, value: Value)]
    @Binding var selection: Value
    var title: String = ""

    var body: some View {
        if KoanTheme.isOn {
            HStack(spacing: 18) {
                ForEach(options, id: \.value) { option in
                    let chosen = option.value == selection
                    Button {
                        selection = option.value
                    } label: {
                        Text(option.label)
                            .font(.koan(.control))
                            .textCase(.lowercase)
                            .foregroundStyle(chosen ? Color.koanInk : Color.koanMuted)
                            .padding(.bottom, 5)
                            .overlay(alignment: .bottom) {
                                if chosen { Rectangle().fill(.tint).frame(height: KoanTheme.hairline) }
                            }
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(chosen ? .isSelected : [])
                }
            }
            .accessibilityElement(children: .contain)
            .accessibilityLabel(title)
            .koanAnimation(KoanTheme.Motion.fast, value: selection)
        } else {
            Picker(title, selection: $selection) {
                ForEach(options, id: \.value) { Text($0.label).tag($0.value) }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
        }
    }
}

private struct KoanRowRole: ViewModifier {
    let selected: Bool
    #if os(macOS)
    @State private var hovering = false
    #endif

    func body(content: Content) -> some View {
        if KoanTheme.isOn {
            content
                .background(fill)
                .koanRule(.bottom)
                #if os(macOS)
                .onHover { hovering = $0 }
                .koanAnimation(KoanTheme.Motion.fast, value: hovering)
                #endif
                #if !os(tvOS)
                .listRowSeparator(.hidden)
                #endif
                .listRowBackground(fill)
        } else {
            content
        }
    }

    private var fill: Color {
        if selected { return .koanSurface }
        #if os(macOS)
        if hovering { return .koanHover.opacity(0.3) }
        #endif
        return .clear
    }
}

private struct KoanSidebarRole: ViewModifier {
    func body(content: Content) -> some View {
        if KoanTheme.isOn {
            #if os(tvOS)
            content.background(Color.koanBg)
            #else
            content
                .scrollContentBackground(.hidden)
                .background(Color.koanBg)
            #endif
        } else {
            content
        }
    }
}

private struct KoanNavRowRole: ViewModifier {
    let selected: Bool

    func body(content: Content) -> some View {
        if KoanTheme.isOn {
            content
                .font(.koan(.body))
                .foregroundStyle(KoanTheme.style(selected ? .accent : .muted))
                .listRowBackground(
                    Rectangle().fill(.clear).overlay(alignment: .leading) {
                        if selected { Rectangle().fill(.tint).frame(width: 2) }
                    }
                )
                .accessibilityAddTraits(selected ? .isSelected : [])
        } else {
            content
        }
    }
}

private struct KoanFocusRole: ViewModifier {
    #if os(tvOS)
    @Environment(\.isFocused) private var focused
    #endif

    func body(content: Content) -> some View {
        #if os(tvOS)
        if KoanTheme.isOn {
            content.koanFocusRing(focused)
        } else {
            content
        }
        #else
        content
        #endif
    }
}

extension View {
    /// The ring tvOS focus draws in the theme: the accent, outside the
    /// control. No lift, no shadow, no glass.
    fileprivate func koanFocusRing(_ on: Bool) -> some View {
        modifier(KoanFocusRing(on: on))
    }
}

/// The accent's own colour rather than `.tint`, which a television never
/// sets: the system's default there is white on white platters. Heavier
/// than a pointer's ring, to be found from across the room.
private struct KoanFocusRing: ViewModifier {
    let on: Bool
    @Environment(\.koanAccent) private var accent

    func body(content: Content) -> some View {
        #if os(tvOS)
        let (width, gap): (CGFloat, CGFloat) = (4, 8)
        #else
        let (width, gap): (CGFloat, CGFloat) = (2, 4)
        #endif
        content.overlay {
            if on {
                Rectangle().strokeBorder(accent.color, lineWidth: width).padding(-gap)
            }
        }
    }
}

private struct KoanBadgeRole: ViewModifier {
    func body(content: Content) -> some View {
        if KoanTheme.isOn {
            content
                .font(.koan(.fine))
                .foregroundStyle(Color.koanInk)
                .padding(.horizontal, 7)
                .padding(.vertical, 2)
                .overlay { Rectangle().strokeBorder(Color.koanRule, lineWidth: KoanTheme.hairline) }
        } else {
            content
                .font(.caption.monospaced())
                .foregroundStyle(.secondary)
                .padding(.horizontal, 8)
                .padding(.vertical, 3)
                .background(.quaternary, in: Capsule())
        }
    }
}

private struct KoanChipRole: ViewModifier {
    func body(content: Content) -> some View {
        if KoanTheme.isOn {
            content
                .padding(.horizontal, 12)
                .padding(.vertical, 6)
                .overlay { Rectangle().strokeBorder(Color.koanMuted, lineWidth: KoanTheme.hairline) }
        } else {
            content
                .padding(.horizontal, 12)
                .padding(.vertical, 6)
                .background(.quaternary, in: Capsule())
        }
    }
}

private struct KoanBarRole: ViewModifier {
    let radius: CGFloat
    let inset: CGFloat

    func body(content: Content) -> some View {
        if KoanTheme.isOn {
            content
                .background(Color.koanBg)
                .koanRule(.top)
        } else {
            content
                .glass(.regular, fallback: .regularMaterial, in: .rect(cornerRadius: radius))
                .padding(.horizontal, inset)
                .padding(.bottom, 14)
        }
    }
}

private struct KoanFormRole: ViewModifier {
    func body(content: Content) -> some View {
        if KoanTheme.isOn {
            #if os(macOS)
            // Grouped, for its layout — footers that wrap, labels and fields
            // in the window's width — with its cards taken away.
            content
                .formStyle(.grouped)
                .scrollContentBackground(.hidden)
                .listRowBackground(Color.clear)
            #elseif os(tvOS)
            // A television's form has no ground or separators to take over.
            content
            #else
            content
                .scrollContentBackground(.hidden)
                .listRowBackground(Color.clear)
                .listRowSeparatorTint(Color.koanRule)
            #endif
        } else {
            content.formStyle(.grouped)
        }
    }
}

private struct KoanListRole: ViewModifier {
    func body(content: Content) -> some View {
        if KoanTheme.isOn {
            #if os(tvOS)
            content
            #else
            content
                .listStyle(.plain)
                .scrollContentBackground(.hidden)
                .listRowBackground(Color.clear)
                .listRowSeparatorTint(Color.koanRule)
                .font(.koan(.body))
            #endif
        } else {
            content
        }
    }
}

private struct KoanFieldRole: ViewModifier {
    #if os(tvOS)
    /// A plain field draws no focus of its own on a television; the ring is
    /// all that says which field the remote is on.
    @FocusState private var focused: Bool
    #endif

    func body(content: Content) -> some View {
        if KoanTheme.isOn {
            content
                .textFieldStyle(.plain)
                .font(.koan(.control))
                .foregroundStyle(Color.koanInk)
                .padding(.horizontal, KoanTheme.Space.m)
                .padding(.vertical, KoanTheme.Space.s)
                .background(Color.koanSurface)
                #if os(tvOS)
                .focused($focused)
                .koanFocusRing(focused)
                #endif
        } else {
            content
        }
    }
}

private struct KoanSectionRole: ViewModifier {
    func body(content: Content) -> some View {
        if KoanTheme.isOn {
            #if os(tvOS)
            content
            #else
            content
                .listRowBackground(Color.clear)
                .listRowSeparatorTint(Color.koanRule)
            #endif
        } else {
            content
        }
    }
}

private struct KoanControlRole: ViewModifier {
    func body(content: Content) -> some View {
        if KoanTheme.isOn {
            content
                .font(.koan(.control))
                .tint(Color.koanInk)
        } else {
            content
        }
    }
}

/// A rule between groups: 1 point of `rule`. The platform's divider otherwise.
struct KoanDivider: View {
    var body: some View {
        if KoanTheme.isOn {
            Rectangle().fill(Color.koanRule).frame(height: KoanTheme.hairline)
        } else {
            Divider()
        }
    }
}

#if !os(tvOS)
private struct KoanToolbarRole: ViewModifier {
    let glass: Bool

    func body(content: Content) -> some View {
        #if os(macOS)
        let bar = ToolbarPlacement.windowToolbar
        #else
        let bar = ToolbarPlacement.navigationBar
        #endif
        if KoanTheme.isOn {
            content
                .toolbarBackground(Color.koanBg, for: bar)
                .toolbarBackgroundVisibility(.visible, for: bar)
        } else {
            content.toolbarBackgroundVisibility(glass ? .hidden : .automatic, for: bar)
        }
    }
}
#endif

extension KoanTheme {
    /// Whether a toolbar item sits on a pane of glass: never in the theme.
    static func pane(_ system: Visibility) -> Visibility { isOn ? .hidden : system }
}

private struct KoanAnimationRole<Value: Equatable>: ViewModifier {
    let animation: Animation
    let value: Value
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func body(content: Content) -> some View {
        content.animation(reduceMotion ? nil : animation, value: value)
    }
}

private struct KoanSheetRole: ViewModifier {
    func body(content: Content) -> some View {
        if KoanTheme.isOn {
            content
                .font(.koan(.body))
                .foregroundStyle(Color.koanInk)
                #if os(macOS)
                .background(Color.koanBg)
                .scrollContentBackground(.hidden)
                #elseif os(tvOS)
                .background(Color.koanBg)
                #else
                .presentationBackground(Color.koanBg)
                .scrollContentBackground(.hidden)
                #endif
        } else {
            content
        }
    }
}

/// One tab of the theme's tab bar: the label in `fine`, lowercase, with its
/// glyph above it when icons are on; the accent and an underline when chosen.
struct KoanTabItem: View {
    let title: String
    let icon: String
    let selected: Bool
    /// Shared by a bar's items, so the underline slides from tab to tab.
    var underline: Namespace.ID?
    @Environment(\.koanIcons) private var icons

    var body: some View {
        VStack(spacing: 4) {
            if icons {
                KoanIcon(icon).font(.system(size: 19))
            }
            Text(title)
                .font(.koan(.fine))
                .textCase(.lowercase)
                .padding(.bottom, 3)
                .overlay(alignment: .bottom) {
                    if selected {
                        let line = Rectangle().fill(.tint).frame(height: KoanTheme.hairline)
                        if let underline {
                            line.matchedGeometryEffect(id: "underline", in: underline)
                        } else {
                            line
                        }
                    }
                }
        }
        .foregroundStyle(KoanTheme.style(selected ? .accent : .muted))
        .frame(maxWidth: .infinity, minHeight: 44)
        .contentShape(Rectangle())
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(title)
        .accessibilityShowsLargeContentViewer {
            KoanIcon(icon)
            Text(title)
        }
        .accessibilityAddTraits(selected ? [.isButton, .isSelected] : .isButton)
    }
}

extension View {
    /// Hides the platform's tab bar where the theme draws its own (iOS).
    func koanHidesSystemTabBar() -> some View {
        modifier(KoanHidesSystemTabBar())
    }
}

/// A form. In the platform's look, a grouped `Form`. In the theme, its
/// sections stacked on the ground, header, rows and footer, with no cards:
/// AppKit's grouped form draws a rounded card behind each section whatever it
/// is told, so the theme does not use one there. iOS and tvOS forms take
/// `.koanForm()` instead, which reaches their rows.
struct KoanForm<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        #if os(macOS)
        if KoanTheme.isOn {
            ScrollView {
                VStack(alignment: .leading, spacing: KoanTheme.Space.l) {
                    content
                }
                .padding(KoanTheme.Space.xl)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .font(.koan(.body))
            .foregroundStyle(Color.koanInk)
            .toggleStyle(KoanToggleStyle())
            .textFieldStyle(.plain)
            .koanButtons(.text)
            .labeledContentStyle(KoanLabeledContentStyle())
        } else {
            Form { content }.formStyle(.grouped)
        }
        #else
        Form { content }.koanForm()
        #endif
    }
}

/// What an empty page says: its glyph, a line, and why. The theme's type and
/// tones; the platform's `ContentUnavailableView` otherwise.
struct KoanUnavailable: View {
    let title: String
    let icon: String
    let detail: String

    init(_ title: String, icon: String, detail: String) {
        self.title = title
        self.icon = icon
        self.detail = detail
    }

    var body: some View {
        if KoanTheme.isOn {
            VStack(spacing: KoanTheme.Space.m) {
                KoanIcon(icon)
                    .font(.system(size: 28))
                    .foregroundStyle(Color.koanMuted)
                Text(title)
                    .font(.koan(.body))
                    .textCase(.lowercase)
                    .foregroundStyle(Color.koanInk)
                Text(detail)
                    .font(.koan(.meta))
                    .foregroundStyle(Color.koanMuted)
                    .multilineTextAlignment(.center)
            }
            .padding(KoanTheme.Space.xxl)
            .frame(maxWidth: 420)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            ContentUnavailableView(title, systemImage: icon, description: Text(detail))
        }
    }
}

/// On a phone, in the theme, the platform's tab bar gives way to the theme's
/// own. An iPad keeps its sidebar layout, the platform's.
private struct KoanHidesSystemTabBar: ViewModifier {
    @Environment(\.horizontalSizeClass) private var width

    func body(content: Content) -> some View {
        #if os(iOS)
        content.toolbar(KoanTheme.isOn && width == .compact ? .hidden : .automatic, for: .tabBar)
        #else
        content
        #endif
    }
}

/// A section's heading: `fine`, `ink`, lowercase, 16 points above. The
/// platform's own heading otherwise.
struct KoanSectionHeader: View {
    let title: String

    init(_ title: String) { self.title = title }

    var body: some View {
        if KoanTheme.isOn {
            Text(title)
                .font(.koan(.fine))
                .foregroundStyle(Color.koanInk)
                .textCase(.lowercase)
                .padding(.top, KoanTheme.Space.l)
                .accessibilityAddTraits(.isHeader)
        } else {
            Text(title)
        }
    }
}

extension View {
    /// A motion token on a change of `value`; none at all with Reduce Motion.
    func koanAnimation(_ animation: Animation, value: some Equatable) -> some View {
        modifier(KoanAnimationRole(animation: animation, value: value))
    }

    /// Lowercase on screen in the theme, as the app's own titles are; the
    /// string, and what VoiceOver and the UI tests read, keep their case.
    func koanCase() -> some View {
        textCase(KoanTheme.isOn ? .lowercase : nil)
    }

    /// What every scene's root carries for the theme: whether icons are drawn,
    /// and the appearance model itself for Settings. A scene inherits nothing
    /// from another, so each root calls this.
    func koanTheme(_ appearance: AppearanceModel) -> some View {
        environment(appearance)
            .environment(\.koanIcons, appearance.showIcons)
    }
}
