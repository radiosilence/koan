import SwiftUI

/// koan's accent.
///
/// The blue everywhere was nothing but SwiftUI's default tint — the system
/// accent colour, which is blue unless the user has changed it. Nobody chose
/// it, and it sits badly against a bone-on-near-black icon.
///
/// Hot pink because the app is otherwise monochrome: the accent is the only
/// colour on screen, so it may as well be one. Pitched bright enough to hold
/// against a near-black window without going fluorescent, and darkened a touch
/// in light mode where the same value would vibrate against white.
extension Color {
    static let koanAccent = Color(
        light: Color(.sRGB, red: 0.85, green: 0.09, blue: 0.45, opacity: 1),
        dark: Color(.sRGB, red: 1.00, green: 0.18, blue: 0.58, opacity: 1)
    )
}

extension Color {
    /// Picks per appearance. SwiftUI has no literal for this without an asset
    /// catalog, and the app has no catalog.
    init(light: Color, dark: Color) {
        self.init(nsColor: NSColor(name: nil) { appearance in
            appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
                ? NSColor(dark) : NSColor(light)
        })
    }
}
