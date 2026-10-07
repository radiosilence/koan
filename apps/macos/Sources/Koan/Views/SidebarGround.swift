#if os(macOS)
import AppKit
import SwiftUI

/// The sidebar column's AppKit chrome, put on the theme's terms.
///
/// macOS 26 wraps the sidebar column in an `NSGlassEffectView` whose glass
/// draws the column's content through itself, so it cannot be hidden. SwiftUI
/// offers no way to turn it off, nor the source list's rounded selection, nor
/// the search field's capsule, so this finds them from inside the column.
///
/// In the theme, the glass is clear and draws nothing of its own, so the
/// sidebar shows the ground behind it: the wash, or the column's flat `bg`.
/// The outline view keeps its selection — what VoiceOver announces and the
/// arrow keys move — but does not draw it; the row draws the theme's mark.
/// The search field is the theme's square field. The platform's look is left
/// as it was.
struct SidebarGround: NSViewRepresentable {
    let themed: Bool

    func makeNSView(context: Context) -> Finder { Finder() }

    func updateNSView(_ view: Finder, context: Context) {
        view.themed = themed
        view.apply()
    }

    final class Finder: NSView {
        var themed = false
        private var original: NSGlassEffectView.Style?

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            apply()
        }

        override func hitTest(_ point: NSPoint) -> NSView? { nil }

        func apply() {
            var view = superview
            while let candidate = view, !(candidate is NSGlassEffectView) { view = candidate.superview }
            guard let glass = view as? NSGlassEffectView else { return }
            if original == nil { original = glass.style }
            glass.style = themed ? .clear : (original ?? .regular)
            let env = ProcessInfo.processInfo.environment
            if env["KOAN_EXP_GLASS"] == "hide" {
                let content = glass.subviews.first?.layer
                glass.layer?.sublayers?.forEach { if $0 !== content { $0.isHidden = themed } }
            }
            if env["KOAN_EXP_GLASS"] == "filters" {
                glass.layer?.filters = themed ? [] : nil
                glass.layer?.backgroundFilters = themed ? [] : nil
            }
            guard themed else { return }
            Self.walk(glass) { view in
                if let outline = view as? NSOutlineView {
                    outline.selectionHighlightStyle = .none
                } else if let field = view as? NSSearchField {
                    Self.square(field)
                }
            }
        }

        private static func walk(_ view: NSView, _ visit: (NSView) -> Void) {
            visit(view)
            view.subviews.forEach { walk($0, visit) }
        }

        private static func square(_ field: NSSearchField) {
            field.isBezeled = false
            field.isBordered = false
            field.drawsBackground = true
            field.backgroundColor = NSColor.koanSurface
            field.focusRingType = .none
            field.font = NSFont.koan(.control)
        }
    }
}
#endif
