import AppKit
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
/// Falls back to the system accent when the catalog is not in the bundle.
/// Compiling it needs `actool`, which ships with Xcode proper rather than the
/// command line tools, so a build made without Xcode has no colour to find.
/// Resolving to nothing tints the app with nothing, which does not merely lose
/// the colour — every borderless control and the playing row's title are drawn
/// in `.tint`, and they become invisible rather than uncoloured.
extension Color {
    static let koanAccent = NSColor(named: "AccentColor").map(Color.init) ?? .accentColor
}
