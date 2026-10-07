#if os(macOS)
import AppKit
import SwiftUI

/// Takes the split view's sidebar glass away while the wash runs under the
/// whole window, so the sidebar is drawn clear over it as every other region
/// is. macOS 26 wraps the sidebar column in an `NSGlassEffectView` whose glass
/// draws the column's content through itself, so it cannot be hidden; SwiftUI
/// offers no way to turn it off, so this sets it to clear glass while the wash
/// runs under the sidebar, and back again when it stops.
struct SidebarGround: NSViewRepresentable {
    let clear: Bool

    func makeNSView(context: Context) -> Finder { Finder() }

    func updateNSView(_ view: Finder, context: Context) {
        view.clear = clear
    }

    final class Finder: NSView {
        var clear = false {
            didSet { if clear != oldValue { apply() } }
        }
        private var original: NSGlassEffectView.Style?

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            apply()
        }

        override func hitTest(_ point: NSPoint) -> NSView? { nil }

        private func apply() {
            var view = superview
            while let candidate = view, !(candidate is NSGlassEffectView) { view = candidate.superview }
            guard let glass = view as? NSGlassEffectView else { return }
            if original == nil { original = glass.style }
            glass.style = clear ? .clear : (original ?? .regular)
        }
    }
}
#endif
