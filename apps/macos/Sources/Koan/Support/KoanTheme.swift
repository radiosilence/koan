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
/// Views name roles — `.koanText(.title)`, `.koanSurface()`, `.koanButton(.prominent)`,
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
        // Navigation titles and subtitles are UIKit's, drawn from the bar's
        // appearances: the system's own two, at rest and at the scroll edge,
        // with the theme's type.
        func themed(_ look: UINavigationBarAppearance) -> UINavigationBarAppearance {
            look.largeTitleTextAttributes = [.font: UIFont.koan(.display), .foregroundColor: UIColor.koanStrong]
            look.titleTextAttributes = [.font: UIFont.koan(.control), .foregroundColor: UIColor.koanStrong]
            look.largeSubtitleTextAttributes = [.font: UIFont.koan(.fine), .foregroundColor: UIColor.koanMuted]
            look.subtitleTextAttributes = [.font: UIFont.koan(.fine), .foregroundColor: UIColor.koanMuted]
            return look
        }
        let rest = UINavigationBarAppearance()
        rest.configureWithDefaultBackground()
        let edge = UINavigationBarAppearance()
        edge.configureWithTransparentBackground()
        let bar = UINavigationBar.appearance()
        bar.standardAppearance = themed(rest)
        bar.compactAppearance = themed(rest.copy())
        bar.scrollEdgeAppearance = themed(edge)
        bar.compactScrollEdgeAppearance = themed(edge.copy())
        #elseif os(tvOS)
        // The tabs across the top and each page's title are UIKit's: the
        // theme's type, the tabs on no ground. The tab titles are lowercased
        // where the tabs are made (`label`).
        let tabs = UITabBarAppearance()
        tabs.configureWithTransparentBackground()
        for item in [tabs.stackedLayoutAppearance, tabs.inlineLayoutAppearance, tabs.compactInlineLayoutAppearance] {
            item.normal.titleTextAttributes = [.font: UIFont.koan(.body), .foregroundColor: UIColor.koanMuted]
            item.selected.titleTextAttributes = [.font: UIFont.koan(.body), .foregroundColor: UIColor.koanInk]
            item.focused.titleTextAttributes = [.font: UIFont.koan(.body)]
        }
        UITabBar.appearance().standardAppearance = tabs
        // A television's navigation bar takes no appearance: setting one is
        // an assertion in UIKit.
        UINavigationBar.appearance().titleTextAttributes = [
            .font: UIFont.koan(.title), .foregroundColor: UIColor.koanStrong,
        ]
        // The search page's field, typed into from the keyboard across the top.
        UITextField.appearance(whenContainedInInstancesOf: [UISearchBar.self]).defaultTextAttributes = [
            .font: UIFont.koan(.title), .foregroundColor: UIColor.koanInk,
        ]
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

    /// How strongly the rule between rows shows: `ink` at this opacity.
    nonisolated static let rowRuleOpacity: CGFloat = 0.12

    /// A title the app writes, lowercased in the theme, for the few places
    /// that take a bare string — navigation titles, AppKit labels. Everywhere
    /// else the case changes only on screen (`.koanCase()`, and the theme's
    /// button and label styles), so accessibility labels, and the UI tests
    /// that find things by them, keep the words as written.
    nonisolated static func label(_ text: String) -> String {
        isOn ? text.lowercased() : text
    }

    /// The title of a tab's own page. On iOS in the theme, none: the tab bar,
    /// or the iPad's sidebar, already shows the name lit, and the theme's way
    /// back is a bare chevron that names nothing. A title that says more than
    /// the tab (a search's query, what the queue follows) is the page's own.
    nonisolated static func tabRootTitle(_ text: String) -> String {
        #if os(iOS)
        isOn ? "" : text
        #else
        label(text)
        #endif
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

    /// "Wash the whole window": on the Mac in the theme, sidebar, toolbar,
    /// transport and lyrics are drawn clear over one wash. Off, they keep
    /// their own grounds. Takes effect at once.
    var washWindow: Bool {
        didSet { if washWindow != oldValue { engine.setWashWindow(on: washWindow) } }
    }

    init(engine: KoanEngine, appearance: Appearance) {
        self.engine = engine
        self.showIcons = appearance.icons
        self.koan = appearance.koan
        self.recordColours = appearance.recordColours
        self.washWindow = appearance.washWindow
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
        #if os(tvOS)
        // The symbols are of different widths; at television size a label's
        // own spacing lets the wide ones touch their titles.
        case (true, .full):
            HStack(spacing: 24) {
                configuration.icon.frame(width: 56)
                configuration.title
            }
        #else
        case (true, .full): Label(configuration).labelStyle(.titleAndIcon)
        #endif
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
    /// The rule between rows: `ink` at low opacity, so it takes on the wash
    /// beneath it instead of drawing a grey grid over it.
    static let koanRowRule = Color.koan(dark: 0xCCCCCC, light: 0x333333, alpha: KoanTheme.rowRuleOpacity)

    fileprivate static func koan(dark: UInt32, light: UInt32, alpha: CGFloat = 1) -> Color {
        #if canImport(AppKit)
        Color(nsColor: NSColor.koan(dark: dark, light: light, alpha: alpha))
        #else
        Color(uiColor: UIColor.koan(dark: dark, light: light, alpha: alpha))
        #endif
    }
}

#if canImport(AppKit)
extension NSColor {
    /// A token, following the appearance it is drawn in.
    static func koan(dark: UInt32, light: UInt32, alpha: CGFloat = 1) -> NSColor {
        NSColor(name: nil) { appearance in
            (appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? NSColor.rgb(dark) : .rgb(light))
                .withAlphaComponent(alpha)
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
    static let koanRowRule = koan(dark: 0xCCCCCC, light: 0x333333, alpha: KoanTheme.rowRuleOpacity)
    /// Hairlines between rows: `ink` at low opacity in the theme.
    @MainActor static var koanSeparator: NSColor { KoanTheme.isOn ? koanRowRule : .separatorColor }
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
    static func koan(dark: UInt32, light: UInt32, alpha: CGFloat = 1) -> UIColor {
        UIColor { ($0.userInterfaceStyle == .dark ? UIColor.rgb(dark) : .rgb(light)).withAlphaComponent(alpha) }
    }

    /// The tokens layer-drawn views read, as on the Mac.
    static let koanInk = koan(dark: 0xCCCCCC, light: 0x333333)
    static let koanStrong = koan(dark: 0xFFFFFF, light: 0x111111)
    static let koanRule = koan(dark: 0x383838, light: 0xE0E0E0)
    static let koanMuted = koan(dark: 0x919191, light: 0x666666)
    @MainActor static var koanQuaternaryLabel: UIColor { KoanTheme.isOn ? koanRule : .quaternaryLabel }
    /// The label colours, as the Mac's: the theme's tokens, or the system's.
    @MainActor static var koanLabel: UIColor { KoanTheme.isOn ? koanInk : .label }
    @MainActor static var koanSecondaryLabel: UIColor { KoanTheme.isOn ? koanMuted : .secondaryLabel }

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
    /// How tall the theme's own tab bar and mini player stand over a phone's
    /// pages, as laid out; zero where the platform's bar is drawn.
    @Entry var koanBarHeight: CGFloat = 0
    /// Rows in a television's form, whose text starts on the headings' edge
    /// with the focus ring out in the margin.
    @Entry var koanRowsBleed = false
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

    /// The weight trait that picks the role's face of the variable Geist Mono
    /// in AppKit and UIKit: Core Text maps `.light` (-0.4) to ExtraLight (200)
    /// and -0.25 to Light (300).
    var faceWeight: CGFloat {
        switch self {
        case .display: -0.4
        case .title, .titleSmall: -0.25
        default: 0
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

    /// A measure that differs between the looks: a thickness, a size, a
    /// margin. The theme's in the theme, the platform's otherwise.
    nonisolated static func metric<T>(_ theme: T, system: T) -> T {
        isOn ? theme : system
    }

    /// Whether the wash runs under the whole window, every region clear over
    /// it ("Wash the whole window"): the Mac, in the theme, unless turned off.
    @MainActor static func washesWindow(_ appearance: AppearanceModel?) -> Bool {
        #if os(macOS)
        isOn && appearance?.washWindow != false
        #else
        false
        #endif
    }

    /// The bare ground of a page or a sheet: `bg` in the theme, `system`
    /// otherwise.
    nonisolated static func ground(_ system: some ShapeStyle) -> AnyShapeStyle {
        isOn ? AnyShapeStyle(Color.koanBg) : AnyShapeStyle(system)
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

#if !os(macOS)
extension UIFont {
    /// A role of the theme's type scale, for UIKit's own drawing (navigation
    /// titles), scaled with Dynamic Type as the role's text style is.
    static func koan(_ role: KoanType) -> UIFont {
        let weight = UIFont.Weight(role.faceWeight)
        // From the family, not from a face's descriptor: a weight added to the
        // Regular face's descriptor keeps the Regular face.
        let wanted = UIFontDescriptor(fontAttributes: [
            .family: "Geist Mono",
            .traits: [UIFontDescriptor.TraitKey.weight: weight],
        ])
        let face = UIFont(name: "Geist Mono", size: role.size) == nil
            ? UIFont.monospacedSystemFont(ofSize: role.size, weight: weight)
            : UIFont(descriptor: wanted, size: role.size)
        let style: UIFont.TextStyle = switch role.scalesWith {
        #if os(tvOS)
        // tvOS has no Large Title style; Title 1 is its largest.
        case .largeTitle: .title1
        #else
        case .largeTitle: .largeTitle
        #endif
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
        let wanted = weight ?? NSFont.Weight(role.faceWeight)
        guard NSFont(name: "Geist Mono", size: role.size) != nil else {
            return .monospacedSystemFont(ofSize: role.size, weight: wanted)
        }
        // From the family, not from a face's descriptor: a weight added to the
        // Regular face's descriptor keeps the Regular face.
        let descriptor = NSFontDescriptor(fontAttributes: [
            .family: "Geist Mono",
            .traits: [NSFontDescriptor.TraitKey.weight: wanted],
        ])
        return NSFont(descriptor: descriptor, size: role.size) ?? .monospacedSystemFont(ofSize: role.size, weight: wanted)
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
    /// rule along its top, full width, over the wash as well. In the
    /// platform's look, a floating slab of glass with the given corner radius,
    /// inset from the window's edges.
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

    /// A material behind a region, out to the edges past the safe area as
    /// `.background(_:)` draws it: `surface` in the theme, which has no
    /// materials; the material otherwise.
    func koanMaterial(_ material: some ShapeStyle) -> some View {
        modifier(KoanMaterialRole(material: AnyShapeStyle(material), shape: nil))
    }

    /// A material in a shape: `surface`, square, in the theme.
    func koanMaterial(_ material: some ShapeStyle, in shape: some Shape) -> some View {
        modifier(KoanMaterialRole(material: AnyShapeStyle(material), shape: AnyShape(shape)))
    }

    /// A popover's content: `bg` beneath it, the popover's own material
    /// replaced. The platform's popover otherwise.
    func koanPopover() -> some View {
        modifier(KoanPopoverRole())
    }

    /// A sheet's chrome: `bg` beneath, no material, the theme's type for
    /// everything that does not set its own.
    func koanSheet() -> some View {
        modifier(KoanSheetRole())
    }

    /// A drop shadow in the platform's look. The theme has none: depth is
    /// rules and surfaces.
    func koanShadow(_ opacity: Double, radius: CGFloat, y: CGFloat = 0) -> some View {
        shadow(color: .black.opacity(KoanTheme.isOn ? 0 : opacity), radius: radius, y: y)
    }
}

enum KoanSurface { case bg, surface }

/// How much a control matters on its screen, as headings do for type: one
/// prominent action per screen or group, the rest standard, and the small
/// actions beside a row compact.
enum KoanButtonKind {
    /// The main thing done here: larger, in the accent, in a square outline.
    /// One per screen or group.
    case prominent
    /// Text and its icon in ink, no outline; the default.
    case standard
    /// Smaller and tighter, for actions beside a row or a title: favourite,
    /// ⋯, revoke, a sheet's lesser actions.
    case compact
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
        case .prominent, .standard, .compact, .text: true
        case .icon, .iconOutlined, .card: false
        }
    }

    /// The label's type role.
    fileprivate var type: KoanType {
        switch self {
        case .prominent: .body
        case .compact: .meta
        default: .control
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
            // One line, always: buttons in a row stand at one height. Truncated
            // rather than pushed past the edge when a label holds a long name;
            // its own size is what it asks for first.
            configuration.label
                .font(.koan(kind.type))
                .textCase(.lowercase)
                .lineLimit(1)
                .layoutPriority(1)
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
        // A destructive action reads as one, whatever its kind.
        if configuration.role == .destructive, kind != .card { return AnyShapeStyle(Color.koanBad) }
        return switch kind {
        case .prominent:
            accent.shade(scheme).readsAsText ? AnyShapeStyle(.tint) : AnyShapeStyle(Color.koanInk)
        case .standard, .compact, .icon, .iconOutlined, .card: AnyShapeStyle(Color.koanInk)
        case .text: AnyShapeStyle(configuration.isPressed ? Color.koanInk : Color.koanMuted)
        }
    }

    private var outline: AnyShapeStyle? {
        switch kind {
        case .prominent: AnyShapeStyle(.tint)
        case .iconOutlined: AnyShapeStyle(Color.koanInk)
        case .standard, .compact, .text, .icon, .card: nil
        }
    }

    private var padding: EdgeInsets {
        switch kind {
        case .prominent: EdgeInsets(top: 12, leading: 20, bottom: 12, trailing: 20)
        case .standard: EdgeInsets(top: 8, leading: 4, bottom: 8, trailing: 4)
        case .compact: EdgeInsets(top: 4, leading: 2, bottom: 4, trailing: 2)
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
        // The small actions beside a title or a row, which a finger still
        // has to land on.
        #if os(iOS)
        case .compact: 44
        #endif
        default: nil
        }
    }
}

private struct KoanToggleRole: ViewModifier {
    func body(content: Content) -> some View {
        if KoanTheme.isOn {
            content.toggleStyle(KoanToggleStyle())
        } else {
            content
        }
    }
}

#if os(macOS)
/// A form row's label in a column of its own, so the fields beside a run of
/// labels start at one edge. The label is a toggle's: `body`, `ink`,
/// lowercase; a label that is data rather than the app's words sets
/// `.textCase(nil)` on its text. A value trails in `control`, `muted`; a field
/// or control in it keeps its own type.
struct KoanLabeledContentStyle: LabeledContentStyle {
    func makeBody(configuration: Configuration) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: KoanTheme.Space.m) {
            configuration.label
                .font(.koan(.body))
                .foregroundStyle(Color.koanInk)
                .textCase(.lowercase)
                .frame(width: 200, alignment: .leading)
            configuration.content
                .font(.koan(.control))
                .foregroundStyle(Color.koanMuted)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}
#endif

#if !os(macOS)
/// A form row on a phone or a television: the label leading in `body` and
/// `ink`, lowercase as every label the app writes is, and the value trailing
/// in `control` and `muted`. A field or control in the value keeps its own
/// type. Without this a row's label takes whatever size the system's form
/// gives a row beside a field. A label that is data (a server's extension, a
/// maker) keeps its case with `.textCase(nil)` on its text.
struct KoanRowLabelStyle: LabeledContentStyle {
    func makeBody(configuration: Configuration) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: KoanTheme.Space.m) {
            configuration.label
                .font(.koan(.body))
                .textCase(.lowercase)
                .foregroundStyle(Color.koanInk)
            Spacer(minLength: 0)
            configuration.content
                .font(.koan(.control))
                .foregroundStyle(Color.koanMuted)
        }
    }
}
#endif

/// A square box: a `muted` outline off, filled with the accent and checked in
/// `bg` on. On a television the row is the button, ringed when focused.
struct KoanToggleStyle: ToggleStyle {
    @Environment(\.isEnabled) private var enabled
    @Environment(\.koanAccent) private var accent
    /// A finger's target on a phone; a pointer needs no more than the row.
    #if os(macOS)
    private static let hit: CGFloat = 24
    #else
    private static let hit: CGFloat = 44
    #endif
    /// The box, at the distance the device is read from.
    #if os(tvOS)
    private static let box: CGFloat = 26
    #else
    private static let box: CGFloat = 14
    #endif

    func makeBody(configuration: Configuration) -> some View {
        Button {
            configuration.isOn.toggle()
        } label: {
            // The label leading and the box trailing, where a switch sits.
            HStack(spacing: KoanTheme.Space.m) {
                configuration.label
                    .font(.koan(.body))
                    .foregroundStyle(Color.koanInk)
                    .textCase(.lowercase)
                    .frame(maxWidth: .infinity, alignment: .leading)
                ZStack {
                    if configuration.isOn {
                        #if os(tvOS)
                        // The accent's own colour: a television sets no tint.
                        Rectangle().fill(accent.color)
                        #else
                        Rectangle().fill(.tint)
                        #endif
                        Image(systemName: "checkmark")
                            .font(.system(size: Self.box * 0.64, weight: .bold))
                            .foregroundStyle(Color.koanBg)
                    } else {
                        Rectangle().strokeBorder(Color.koanMuted, lineWidth: KoanTheme.hairline)
                    }
                }
                .frame(width: Self.box, height: Self.box)
            }
            .frame(minHeight: Self.hit)
            .contentShape(Rectangle())
        }
        #if os(tvOS)
        .buttonStyle(TelevisionRow())
        #else
        .buttonStyle(.plain)
        .opacity(enabled ? 1 : 0.4)
        #endif
        .koanAnimation(KoanTheme.Motion.fast, value: configuration.isOn)
        .accessibilityValue(configuration.isOn ? "On" : "Off")
        .accessibilityAddTraits(.isToggle)
    }
}

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

/// A pop-up picker. In the theme on the Mac and a phone, a menu whose label
/// is the chosen option in `control` type and `ink`: AppKit's pop-up button and
/// UIKit's picker draw their own value in the system face, on the Mac in a
/// rounded bezel, whatever the environment says. Options the app writes are
/// lowercased; `keepsCase` keeps options that are data, such as device and
/// preset names. On a television in the theme, a row naming the choice that
/// opens the options as a sheet of rows: the system's menu is a popover of
/// grey pills. The system picker, through `.koanControl()`, everywhere else.
struct KoanPicker<Value: Hashable>: View {
    let title: String
    @Binding var selection: Value
    let options: [(label: String, value: Value)]
    let keepsCase: Bool

    init(_ title: String, selection: Binding<Value>, options: [(label: String, value: Value)], keepsCase: Bool = false) {
        self.title = title
        _selection = selection
        self.options = options
        self.keepsCase = keepsCase
    }

    var body: some View {
        #if os(tvOS)
        if KoanTheme.isOn {
            TelevisionPicker(title: title, selection: $selection, options: options.map { (shown($0.label), $0.value) })
        } else {
            picker
        }
        #else
        if KoanTheme.isOn {
            LabeledContent {
                Menu {
                    Picker(title, selection: $selection) {
                        ForEach(options, id: \.value) { Text(shown($0.label)).textCase(nil).tag($0.value) }
                    }
                    .pickerStyle(.inline)
                } label: {
                    HStack(spacing: KoanTheme.Space.xs) {
                        // Cased by `shown` alone: a menu's label takes the
                        // case of the button style around it.
                        Text(shown(options.first { $0.value == selection }?.label ?? ""))
                            .textCase(nil)
                            .lineLimit(1)
                        KoanIcon("chevron.up.chevron.down").font(.koan(.fine))
                    }
                    .font(.koan(.control))
                    .foregroundStyle(Color.koanInk)
                }
                #if os(macOS)
                .menuStyle(.button)
                .buttonStyle(.plain)
                .menuIndicator(.hidden)
                .fixedSize()
                #endif
                .tint(Color.koanInk)
            } label: {
                Text(title)
            }
        } else {
            picker
        }
        #endif
    }

    private func shown(_ label: String) -> String {
        keepsCase ? label : KoanTheme.label(label)
    }

    private var picker: some View {
        Picker(title, selection: $selection) {
            ForEach(options, id: \.value) { Text($0.label).tag($0.value) }
        }
        .koanControl()
    }
}

/// A choice from a list long enough to want a page of its own, each option
/// drawn by `row`, in sections. On iOS in the theme, a row that pushes the
/// options as a page that keeps clear of the theme's tab bar: the page
/// SwiftUI pushes for a `.navigationLink` picker is its own, and nothing can
/// give it the bar's inset, so its last rows sat under the mini player. The
/// platform's picker otherwise, pushed on iOS as before.
struct KoanListPicker<Value: Hashable, Row: View>: View {
    let title: String
    @Binding var selection: Value
    let sections: [(title: String?, values: [Value])]
    /// What the chosen value is called, beside the title.
    let name: (Value) -> String
    @ViewBuilder let row: (Value) -> Row

    var body: some View {
        #if os(iOS)
        if KoanTheme.isOn {
            NavigationLink {
                KoanListPickerPage(title: title, selection: $selection, sections: sections, row: row)
            } label: {
                LabeledContent(title) {
                    Text(name(selection))
                        .koanText(.control, .muted)
                        .lineLimit(1)
                }
            }
        } else {
            picker.pickerStyle(.navigationLink)
        }
        #else
        picker
        #endif
    }

    private var picker: some View {
        Picker(title, selection: $selection) {
            ForEach(sections.indices, id: \.self) { i in
                if let heading = sections[i].title {
                    Section(KoanTheme.label(heading)) { options(sections[i].values) }
                } else {
                    options(sections[i].values)
                }
            }
        }
    }

    private func options(_ values: [Value]) -> some View {
        ForEach(values, id: \.self) { row($0).tag($0) }
    }
}

#if os(iOS)
/// The options of a `KoanListPicker`, as a page: the chosen one ticked, and
/// back to the page before on a choice, as the platform's picker goes.
private struct KoanListPickerPage<Value: Hashable, Row: View>: View {
    let title: String
    @Binding var selection: Value
    let sections: [(title: String?, values: [Value])]
    @ViewBuilder let row: (Value) -> Row
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        KoanForm {
            ForEach(sections.indices, id: \.self) { i in
                Section {
                    ForEach(sections[i].values, id: \.self) { value in
                        Button {
                            selection = value
                            dismiss()
                        } label: {
                            HStack(spacing: KoanTheme.Space.m) {
                                row(value)
                                Spacer(minLength: 0)
                                if value == selection {
                                    KoanIcon("checkmark")
                                }
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityAddTraits(value == selection ? .isSelected : [])
                    }
                } header: {
                    if let heading = sections[i].title {
                        KoanSectionHeader(KoanTheme.label(heading))
                    }
                }
            }
        }
        .navigationTitle(KoanTheme.label(title))
        .koanBackButton()
        .koanHidesSystemTabBar()
    }
}
#endif

#if os(tvOS)
private struct TelevisionPicker<Value: Hashable>: View {
    let title: String
    @Binding var selection: Value
    let options: [(label: String, value: Value)]
    @State private var open = false

    var body: some View {
        Button { open = true } label: {
            HStack(spacing: KoanTheme.Space.m) {
                Text(title)
                    .textCase(.lowercase)
                Spacer(minLength: 0)
                Text(options.first { $0.value == selection }?.label ?? "")
                    .textCase(nil)
                    .font(.koan(.control))
                    .foregroundStyle(Color.koanMuted)
            }
        }
        .buttonStyle(TelevisionRow())
        .accessibilityValue(options.first { $0.value == selection }?.label ?? "")
        .televisionPanel(isPresented: $open, title: title) {
            TelevisionChoices(selection: $selection, options: options) { open = false }
        }
    }
}

/// A choice of options as the theme's rows, the chosen one ticked: what a
/// picker or a menu of choices opens on a television.
struct TelevisionChoices<Value: Hashable>: View {
    @Binding var selection: Value
    let options: [(label: String, value: Value)]
    let chosen: () -> Void

    var body: some View {
        ForEach(options, id: \.value) { option in
            Button {
                selection = option.value
                chosen()
            } label: {
                HStack(spacing: KoanTheme.Space.m) {
                    Text(option.label).textCase(nil)
                    Spacer(minLength: 0)
                    if option.value == selection {
                        KoanIcon("checkmark")
                    }
                }
            }
            .buttonStyle(TelevisionRow())
            .accessibilityAddTraits(option.value == selection ? .isSelected : [])
        }
    }
}
#endif

#if !os(tvOS)
/// A slider as the theme draws it: a 1-point `rule` track, a 3-point accent
/// fill up to the value, and a square 8 × 8 `ink` thumb shown only on hover,
/// focus or drag, in a 44-point hit area. The system's slider in the
/// platform's look, and to assistive technologies in both.
struct KoanSlider<End: View>: View {
    let title: String
    @Binding var value: Double
    let range: ClosedRange<Double>
    let step: Double?
    let editing: (Bool) -> Void
    @ViewBuilder let low: () -> End
    @ViewBuilder let high: () -> End
    @State private var hovering = false
    @State private var dragging = false
    @FocusState private var focused: Bool
    @Environment(\.isEnabled) private var enabled

    init(
        _ title: String,
        value: Binding<Double>,
        in range: ClosedRange<Double>,
        step: Double? = nil,
        onEditingChanged editing: @escaping (Bool) -> Void = { _ in },
        @ViewBuilder low: @escaping () -> End,
        @ViewBuilder high: @escaping () -> End
    ) {
        self.title = title
        _value = value
        self.range = range
        self.step = step
        self.editing = editing
        self.low = low
        self.high = high
    }

    var body: some View {
        if KoanTheme.isOn {
            HStack(spacing: KoanTheme.Space.s) {
                low()
                track
                high()
            }
            .opacity(enabled ? 1 : 0.4)
            .accessibilityRepresentation { system }
        } else {
            system
        }
    }

    private var system: some View {
        Group {
            if let step {
                Slider(
                    value: $value, in: range, step: step,
                    label: { Text(title) }, minimumValueLabel: { low() }, maximumValueLabel: { high() },
                    onEditingChanged: editing
                )
            } else {
                Slider(
                    value: $value, in: range,
                    label: { Text(title) }, minimumValueLabel: { low() }, maximumValueLabel: { high() },
                    onEditingChanged: editing
                )
            }
        }
    }

    private var fraction: Double {
        guard range.upperBound > range.lowerBound else { return 0 }
        return ((value - range.lowerBound) / (range.upperBound - range.lowerBound)).clamped()
    }

    private var track: some View {
        GeometryReader { geo in
            let x = geo.size.width * fraction
            ZStack(alignment: .leading) {
                Rectangle().fill(Color.koanRule).frame(height: KoanTheme.hairline)
                Rectangle().fill(.tint).frame(width: x, height: 3)
                if hovering || dragging || focused {
                    Rectangle()
                        .fill(Color.koanInk)
                        .frame(width: 8, height: 8)
                        .offset(x: min(max(x - 4, 0), geo.size.width - 8))
                }
            }
            .frame(maxHeight: .infinity)
            .contentShape(Rectangle())
            .gesture(
                DragGesture(minimumDistance: 0)
                    .onChanged { drag in
                        if !dragging {
                            dragging = true
                            editing(true)
                        }
                        set(drag.location.x / max(geo.size.width, 1))
                    }
                    .onEnded { drag in
                        set(drag.location.x / max(geo.size.width, 1))
                        dragging = false
                        editing(false)
                    }
            )
        }
        .frame(height: 44)
        .onHover { hovering = $0 }
        .focusable()
        .focused($focused)
        .focusEffectDisabled()
        #if os(macOS)
        .onMoveCommand { direction in
            let by = step ?? (range.upperBound - range.lowerBound) / 20
            switch direction {
            case .left, .down: move(to: value - by)
            case .right, .up: move(to: value + by)
            default: break
            }
        }
        #endif
        .koanAnimation(KoanTheme.Motion.fast, value: hovering || dragging || focused)
    }

    private func set(_ share: Double) {
        move(to: range.lowerBound + share.clamped() * (range.upperBound - range.lowerBound))
    }

    private func move(to raw: Double) {
        var next = min(max(raw, range.lowerBound), range.upperBound)
        if let step, step > 0 {
            next = range.lowerBound + ((next - range.lowerBound) / step).rounded() * step
        }
        if next != value { value = next }
    }
}

extension KoanSlider where End == EmptyView {
    init(
        _ title: String,
        value: Binding<Double>,
        in range: ClosedRange<Double>,
        step: Double? = nil,
        onEditingChanged editing: @escaping (Bool) -> Void = { _ in }
    ) {
        self.init(title, value: value, in: range, step: step, onEditingChanged: editing, low: { EmptyView() }, high: { EmptyView() })
    }
}
#endif

#if !os(tvOS)
/// A stepper as the theme draws it: the label leading in `body`, then − and +
/// as square `muted` outlines at the row's trailing edge, each within a
/// 44-point hit area on a phone. The system's stepper in the platform's look.
/// tvOS has no stepper.
struct KoanStepper<Value: Strideable>: View {
    let title: String
    @Binding var value: Value
    let range: ClosedRange<Value>
    let step: Value.Stride
    @Environment(\.isEnabled) private var enabled
    #if os(macOS)
    private static var hit: CGFloat { 24 }
    #else
    private static var hit: CGFloat { 44 }
    #endif

    init(_ title: String, value: Binding<Value>, in range: ClosedRange<Value>, step: Value.Stride = 1) {
        self.title = title
        _value = value
        self.range = range
        self.step = step
    }

    var body: some View {
        if KoanTheme.isOn {
            HStack(spacing: KoanTheme.Space.xs) {
                Text(title)
                    .font(.koan(.body))
                    .foregroundStyle(Color.koanInk)
                    .textCase(.lowercase)
                    .frame(maxWidth: .infinity, alignment: .leading)
                button("minus", by: -step, allowed: value > range.lowerBound)
                button("plus", by: step, allowed: value < range.upperBound)
            }
            .opacity(enabled ? 1 : 0.4)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(title)
            .accessibilityAdjustableAction { direction in
                switch direction {
                case .increment: move(by: step)
                case .decrement: move(by: -step)
                @unknown default: break
                }
            }
        } else {
            Stepper(title, value: $value, in: range, step: step).koanControl()
        }
    }

    private func button(_ symbol: String, by delta: Value.Stride, allowed: Bool) -> some View {
        Button { move(by: delta) } label: {
            KoanIcon(symbol)
                .font(.system(size: 11))
                .foregroundStyle(Color.koanInk)
                .frame(width: 28, height: 28)
                .overlay { Rectangle().strokeBorder(Color.koanMuted, lineWidth: KoanTheme.hairline) }
                .frame(minWidth: Self.hit, minHeight: Self.hit)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!allowed)
        .opacity(allowed ? 1 : 0.4)
    }

    private func move(by delta: Value.Stride) {
        value = min(max(value.advanced(by: delta), range.lowerBound), range.upperBound)
    }
}
#endif

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
    @Environment(AppearanceModel.self) private var appearance: AppearanceModel?

    func body(content: Content) -> some View {
        if KoanTheme.isOn {
            #if os(tvOS)
            content.background(Color.koanBg)
            #else
            content
                .scrollContentBackground(.hidden)
                .background(KoanTheme.washesWindow(appearance) ? Color.clear : Color.koanBg)
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
    /// The ring tvOS focus draws in the theme: 2 points of the accent, `gap`
    /// outside the control, or on its edge for a row that runs the width of
    /// the page. No lift, no shadow, no glass.
    func koanFocusRing(_ on: Bool, gap: CGFloat? = nil) -> some View {
        modifier(KoanFocusRing(on: on, gap: gap))
    }
}

/// The accent's own colour rather than `.tint`, which a television never
/// sets: the system's default there is white on white platters.
private struct KoanFocusRing: ViewModifier {
    let on: Bool
    let gap: CGFloat?
    @Environment(\.koanAccent) private var accent

    func body(content: Content) -> some View {
        #if os(tvOS)
        let standoff = gap ?? 8
        #else
        let standoff = gap ?? 4
        #endif
        content.overlay {
            if on {
                Rectangle().strokeBorder(accent.color, lineWidth: 2).padding(-standoff)
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
                .font(.koan(.body))
                .foregroundStyle(Color.koanInk)
            #else
            // Rows give up their ground through `washedRow`, on the content.
            content
                .scrollContentBackground(.hidden)
                .font(.koan(.body))
                .foregroundStyle(Color.koanInk)
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
            content.font(.koan(.body))
            #else
            // Rows give up their ground through `washedRow`, on the content.
            content
                .listStyle(.plain)
                .scrollContentBackground(.hidden)
                .listRowSeparatorTint(Color.koanRowRule)
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
    @Environment(AppearanceModel.self) private var appearance: AppearanceModel?

    func body(content: Content) -> some View {
        #if os(macOS)
        let bar = ToolbarPlacement.windowToolbar
        #else
        let bar = ToolbarPlacement.navigationBar
        #endif
        if KoanTheme.washesWindow(appearance) {
            // No ground and no edge: the wash runs under the toolbar, and the
            // page stops below it rather than fading out beneath it (see
            // `clearsTransport`).
            content
                .toolbarBackgroundVisibility(.hidden, for: bar)
                .scrollEdgeEffectHidden(true, for: .top)
        } else if KoanTheme.isOn {
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

private struct KoanMaterialRole: ViewModifier {
    let material: AnyShapeStyle
    let shape: AnyShape?

    func body(content: Content) -> some View {
        switch (KoanTheme.isOn, shape) {
        case (true, nil): content.background(Color.koanSurface)
        case (true, .some): content.background(Color.koanSurface, in: Rectangle())
        case (false, nil): content.background(material)
        case (false, .some(let shape)): content.background(material, in: shape)
        }
    }
}

private struct KoanPopoverRole: ViewModifier {
    func body(content: Content) -> some View {
        if KoanTheme.isOn {
            content
                .background(Color.koanBg)
                .presentationBackground(Color.koanBg)
        } else {
            content
        }
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
                // Only where it is presented: a settings page takes this too,
                // and its ground is the wash.
                .presentationBackground(Color.koanBg)
                #else
                .presentationBackground(Color.koanBg)
                .presentationCornerRadius(0)
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
    /// Where the tab sits in its bar, for VoiceOver: "tab 2 of 4", as the
    /// platform's tab bar says it.
    var position: (index: Int, count: Int)?
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
        .accessibilityValue(position.map { "Tab \($0.index + 1) of \($0.count)" } ?? "")
    }
}

extension View {
    /// A pushed page's way back. In the theme on iOS, a bare chevron in `ink`
    /// in place of the platform's glass circle, which no bar appearance
    /// reaches; the edge swipe still goes back. The platform's back button
    /// otherwise.
    @ViewBuilder
    func koanBackButton() -> some View {
        #if os(iOS)
        modifier(KoanBackButton())
        #else
        self
        #endif
    }
}

#if os(iOS)

private struct KoanBackButton: ViewModifier {
    @Environment(\.dismiss) private var dismiss

    func body(content: Content) -> some View {
        if KoanTheme.isOn {
            content
                .navigationBarBackButtonHidden()
                .toolbar {
                    ToolbarItem(placement: .topBarLeading) {
                        Button { dismiss() } label: {
                            KoanIcon("chevron.left")
                                .font(.koan(.titleSmall))
                                .foregroundStyle(Color.koanInk)
                                .frame(width: 44, height: 44, alignment: .leading)
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Back")
                    }
                    .sharedBackgroundVisibility(.hidden)
                }
                .background { SwipeBack() }
        } else {
            content
        }
    }
}

/// The edge swipe a hidden back button takes with it, given back: the stack's
/// pop gesture, answered by a delegate that allows it wherever there is a page
/// to go back to.
private struct SwipeBack: UIViewControllerRepresentable {
    func makeUIViewController(context: Context) -> Controller { Controller() }
    func updateUIViewController(_ controller: Controller, context: Context) {}

    final class Controller: UIViewController {
        override func viewDidAppear(_ animated: Bool) {
            super.viewDidAppear(animated)
            navigationController?.interactivePopGestureRecognizer?.delegate = Allow.shared
        }
    }

    /// One, held for good: the gesture holds its delegate weakly, and outlives
    /// every page that set it.
    @MainActor final class Allow: NSObject, UIGestureRecognizerDelegate {
        static let shared = Allow()

        func gestureRecognizerShouldBegin(_ gesture: UIGestureRecognizer) -> Bool {
            ((gesture.view?.next as? UINavigationController)?.viewControllers.count ?? 0) > 1
        }
    }
}
#endif

extension View {
    /// A page's search field. In the theme on iOS, a flat `surface` field under
    /// the title with a bare clear button, in place of the platform's glass
    /// capsule and its glass close button. `.searchable` everywhere else.
    @ViewBuilder
    func koanSearchable(
        text: Binding<String>,
        placement: SearchFieldPlacement = .automatic,
        prompt: String,
        onSubmit: @escaping () -> Void = {}
    ) -> some View {
        #if os(iOS)
        if KoanTheme.isOn {
            safeAreaInset(edge: .top, spacing: 0) {
                KoanSearchField(text: text, prompt: prompt, onSubmit: onSubmit)
                    .padding(.horizontal, KoanTheme.Space.page)
                    .padding(.vertical, KoanTheme.Space.s)
            }
        } else {
            searchable(text: text, placement: placement, prompt: prompt)
                .onSubmit(of: .search, onSubmit)
        }
        #else
        searchable(text: text, placement: placement, prompt: KoanTheme.label(prompt))
            .onSubmit(of: .search, onSubmit)
        #endif
    }
}

#if os(iOS)
/// The theme's search field: a glyph, the field and, once there is something
/// to clear, a bare clear button, on `surface`.
private struct KoanSearchField: View {
    @Binding var text: String
    let prompt: String
    let onSubmit: () -> Void

    var body: some View {
        HStack(spacing: KoanTheme.Space.s) {
            KoanIcon(Icon.search)
                .foregroundStyle(Color.koanMuted)
                .accessibilityHidden(true)
            TextField(KoanTheme.label(prompt), text: $text)
                .submitLabel(.search)
                .onSubmit(onSubmit)
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
                .accessibilityLabel(prompt)
            if !text.isEmpty {
                Button { text = "" } label: {
                    KoanIcon("xmark")
                        .foregroundStyle(Color.koanMuted)
                        .frame(minWidth: 28, minHeight: 28)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Clear")
            }
        }
        .koanField()
    }
}
#endif

extension View {
    /// Hides the platform's tab bar where the theme draws its own (iOS), and
    /// makes the page room for that bar at its foot. Applied to every page in
    /// a tab, root and pushed alike: an inset from outside the tab view does
    /// not reach a list inside a tab's stack in every case, and a list that
    /// misses it stops scrolling with its last rows under the bar.
    func koanHidesSystemTabBar() -> some View {
        modifier(KoanHidesSystemTabBar())
    }
}

/// A form. In the platform's look, a grouped `Form`. In the theme, its
/// sections stacked on the ground, header, rows and footer, with no cards:
/// AppKit's grouped form draws a rounded card behind each section whatever it
/// is told, so the theme does not use one there. On iOS the theme's form is a
/// grouped list: sections full width and square, rows on the ground, rules at
/// one inset. tvOS forms take `.koanForm()`.
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
        #elseif os(iOS)
        if KoanTheme.isOn {
            List { Group { content }.washedRow() }
                .listStyle(.grouped)
                .koanForm()
                .labeledContentStyle(KoanRowLabelStyle())
        } else {
            Form { content }.koanForm()
        }
        #else
        // A television's `Form` draws every row on a grey platter and the
        // focused one white, whatever it is told; in the theme the sections
        // stack on the ground, as on the Mac, and each row is a control of
        // the theme's.
        if KoanTheme.isOn {
            ScrollView {
                VStack(alignment: .leading, spacing: KoanTheme.Space.l) {
                    content
                }
                .padding(.vertical, KoanTheme.Space.xl)
                .padding(.horizontal, 20)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .environment(\.koanRowsBleed, true)
            .koanForm()
            .toggleStyle(KoanToggleStyle())
            .buttonStyle(TelevisionRow())
            .labeledContentStyle(KoanRowLabelStyle())
        } else {
            Form { content }.koanForm()
        }
        #endif
    }
}

/// What an empty page says: its glyph, a line, and why. The theme's type and
/// tones; the platform's `ContentUnavailableView` otherwise.
struct KoanUnavailable: View {
    let title: String
    let icon: String
    let detail: String?

    init(_ title: String, icon: String, detail: String?) {
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
                if let detail {
                    Text(detail)
                        .font(.koan(.meta))
                        .foregroundStyle(Color.koanMuted)
                        .multilineTextAlignment(.center)
                }
            }
            .padding(KoanTheme.Space.xxl)
            // Room for a title on one line, at the size the device is read at.
            .frame(maxWidth: 420 * KoanType.body.size / 15)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            ContentUnavailableView(title, systemImage: icon, description: detail.map(Text.init))
        }
    }
}

/// In the theme the platform's tab bar gives way to the theme's own: the tabs
/// and mini player on a phone, the sidebar and mini player on an iPad. Every
/// page in a tab takes this, so it also gives the navigation bar its ground.
private struct KoanHidesSystemTabBar: ViewModifier {
    @Environment(\.koanBarHeight) private var bar

    func body(content: Content) -> some View {
        #if os(iOS)
        if KoanTheme.isOn {
            content
                // The page passes under the navigation bar behind a hard edge,
                // rather than the platform's blur with it showing through. A
                // bar background set here would replace the bar's appearance,
                // and its Geist Mono titles with it.
                .scrollEdgeEffectStyle(.hard, for: .top)
                .toolbar(.hidden, for: .tabBar)
                .safeAreaInset(edge: .bottom, spacing: 0) {
                    Color.clear.frame(height: bar)
                }
                // Room under the last row, so a page scrolled to its end
                // stops short of the bar's rule rather than against it. On
                // the scroll content alone: a bar a page pins to its foot
                // still meets the mini player.
                .contentMargins(.bottom, KoanTheme.Space.xxl, for: .scrollContent)
                // The bar draws its own ground. The platform's edge effect
                // would otherwise paint the inset as a grey band over the
                // page's last rows, where content should pass under the bar.
                .scrollEdgeEffectHidden(true, for: .bottom)
        } else {
            content
        }
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
