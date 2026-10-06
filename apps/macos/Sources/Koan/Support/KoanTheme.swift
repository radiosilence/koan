#if canImport(AppKit)
import AppKit
#else
import UIKit
#endif
import CoreText
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
    /// The kōan theme, rather than the platform's look.
    private(set) static var isOn = false

    static func apply(_ appearance: Appearance) {
        isOn = appearance.koan
        if isOn { registerFace() }
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

    /// Text the app writes for itself (navigation, headings, buttons) as the
    /// theme sets it: lowercase. Text from the library keeps its own case.
    static func label(_ text: String) -> String {
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

    init(engine: KoanEngine, appearance: Appearance) {
        self.engine = engine
        self.showIcons = appearance.icons
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
                Text(KoanTheme.label(title))
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
    case ink, strong, muted, accent, bad
}

// MARK: - Accent

/// The accent for a record, tone-mapped as the spec sets out: the sleeve's hue
/// kept, its lightness and chroma moved into a band per appearance in OKLCH,
/// clear of `bad`'s hue. Mint when there is no record or no usable hue.
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

    var color: Color {
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
    /// is no record, or none with a colour.
    init(record: Color?) {
        guard let record else { self = .mint; return }
        let resolved = record.resolve(in: EnvironmentValues())
        let (_, c, h) = OKLCH.from(
            linear: (Double(resolved.linearRed), Double(resolved.linearGreen), Double(resolved.linearBlue))
        )
        guard c >= Self.noHue,
              let dark = Self.shade(hue: h, chroma: c, band: Self.darkBand, bad: 0xEF6B73, bg: 0x1E1E1E, surface: 0x2A2A2A),
              let light = Self.shade(hue: h, chroma: c, band: Self.lightBand, bad: 0xC43F3F, bg: 0xFFFFFF, surface: 0xF2F2F2)
        else { self = .mint; return }
        self.dark = dark
        self.light = light
    }

    private init(dark: Shade, light: Shade) {
        self.dark = dark
        self.light = light
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

    var size: CGFloat {
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
}

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

    /// One of the theme's buttons. In the platform's look, the nearest system
    /// style.
    func koanButton(_ kind: KoanButtonKind) -> some View {
        modifier(KoanButtonRole(kind: kind))
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

    /// Focus on tvOS, as the theme shows it: a ring in the accent. Elsewhere, and
    /// in the platform's look, the system's own.
    func koanFocus() -> some View {
        modifier(KoanFocusRole())
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
        case .accent: accent.shade(scheme).readsAsText ? AnyShapeStyle(.tint) : AnyShapeStyle(Color.koanInk)
        }
    }

    private var system: AnyShapeStyle {
        switch tone {
        case .ink, .strong: AnyShapeStyle(.primary)
        case .muted: AnyShapeStyle(.secondary)
        case .accent: AnyShapeStyle(.tint)
        case .bad: AnyShapeStyle(.red)
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

private struct KoanButtonRole: ViewModifier {
    let kind: KoanButtonKind

    func body(content: Content) -> some View {
        if KoanTheme.isOn {
            content.buttonStyle(KoanButtonStyle(kind: kind))
        } else {
            switch kind {
            case .primary: content.buttonStyle(.borderedProminent)
            case .secondary: content.buttonStyle(.bordered)
            case .text, .icon, .iconOutlined: content.buttonStyle(.borderless)
            }
        }
    }
}

/// The theme's buttons: square, outlined or bare, pressed to `hover`, with no
/// motion of their own.
struct KoanButtonStyle: ButtonStyle {
    let kind: KoanButtonKind
    @Environment(\.isEnabled) private var enabled
    @Environment(\.koanAccent) private var accent
    @Environment(\.colorScheme) private var scheme
    #if os(tvOS)
    @Environment(\.isFocused) private var focused
    #endif

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.koan(.control))
            .foregroundStyle(foreground(configuration))
            .padding(padding)
            .frame(minWidth: hit, minHeight: hit)
            .background(configuration.isPressed ? Color.koanHover : .clear)
            .overlay {
                if let outline {
                    Rectangle().strokeBorder(outline, lineWidth: KoanTheme.hairline)
                }
            }
            .contentShape(Rectangle())
            .opacity(enabled ? 1 : 0.4)
            .koanFocusRing(focusedNow)
    }

    private var focusedNow: Bool {
        #if os(tvOS)
        focused
        #else
        false
        #endif
    }

    private func foreground(_ configuration: Configuration) -> AnyShapeStyle {
        switch kind {
        case .primary:
            accent.shade(scheme).readsAsText ? AnyShapeStyle(.tint) : AnyShapeStyle(Color.koanInk)
        case .secondary, .icon, .iconOutlined: AnyShapeStyle(Color.koanInk)
        case .text: AnyShapeStyle(configuration.isPressed ? Color.koanInk : Color.koanMuted)
        }
    }

    private var outline: AnyShapeStyle? {
        switch kind {
        case .primary: AnyShapeStyle(.tint)
        case .secondary: AnyShapeStyle(Color.koanMuted)
        case .iconOutlined: AnyShapeStyle(Color.koanInk)
        case .text, .icon: nil
        }
    }

    private var padding: EdgeInsets {
        switch kind {
        case .primary, .secondary: EdgeInsets(top: 10, leading: 16, bottom: 10, trailing: 16)
        case .text: EdgeInsets(top: 4, leading: 0, bottom: 4, trailing: 0)
        case .icon, .iconOutlined: EdgeInsets()
        }
    }

    private var hit: CGFloat? {
        switch kind {
        case .icon, .iconOutlined: 44
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
/// A square box: a `muted` outline off, filled with the accent and checked in
/// `bg` on.
struct KoanToggleStyle: ToggleStyle {
    @Environment(\.isEnabled) private var enabled

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
            .frame(minHeight: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .opacity(enabled ? 1 : 0.4)
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
                        Text(KoanTheme.label(option.label))
                            .font(.koan(.control))
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
    /// The ring tvOS focus draws in the theme: 2 points of the accent, outside
    /// the control. No lift, no shadow, no glass.
    fileprivate func koanFocusRing(_ on: Bool) -> some View {
        overlay {
            if on {
                Rectangle().strokeBorder(.tint, lineWidth: 2).padding(-4)
            }
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
                #else
                .presentationBackground(Color.koanBg)
                #endif
                .scrollContentBackground(.hidden)
        } else {
            content
        }
    }
}

/// A section's heading: `fine`, `ink`, lowercase, 16 points above. The
/// platform's own heading otherwise.
struct KoanSectionHeader: View {
    let title: String

    init(_ title: String) { self.title = title }

    var body: some View {
        if KoanTheme.isOn {
            Text(KoanTheme.label(title))
                .font(.koan(.fine))
                .foregroundStyle(Color.koanInk)
                .textCase(nil)
                .padding(.top, KoanTheme.Space.l)
                .accessibilityAddTraits(.isHeader)
        } else {
            Text(title)
        }
    }
}

extension View {
    /// What every scene's root carries for the theme: whether icons are drawn,
    /// and the appearance model itself for Settings. A scene inherits nothing
    /// from another, so each root calls this.
    func koanTheme(_ appearance: AppearanceModel) -> some View {
        environment(appearance)
            .environment(\.koanIcons, appearance.showIcons)
    }
}
