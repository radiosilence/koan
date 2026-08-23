import SwiftUI

/// koan's accent, read from the asset catalog.
///
/// The catalog is the source of truth rather than a literal here, because
/// AppKit needs it there: list selection, focus rings and control tints come
/// from the app's declared accent colour, and nothing in SwiftUI can override
/// them. `.tint` reaches SwiftUI's own drawing and stops at the edge of every
/// AppKit-backed control, which is why the sidebar stayed blue however the app
/// was tinted.
///
/// `NSAccentColorName` in the bundle's Info.plist points at this colour set;
/// see the `macos-bundle` recipe.
extension Color {
    static let koanAccent = Color("AccentColor")
}
