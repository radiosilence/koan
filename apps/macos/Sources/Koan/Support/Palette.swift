import SwiftUI

/// koan's accent, taken from the app icon.
///
/// The blue everywhere was nothing but SwiftUI's default tint — the system
/// accent colour, which is blue unless the user has changed it. Nobody chose
/// it, and it sits badly against a bone-on-near-black icon.
///
/// Neutral grey rather than a hue: koan's icon is bone on near-black and the
/// app is otherwise monochrome, so any colour here would be the only one on
/// screen. Light enough to read as an accent against a dark window, dark enough
/// not to be mistaken for plain white text.
extension Color {
    static let koanAccent = Color(.sRGB, white: 0.78, opacity: 1)
}
