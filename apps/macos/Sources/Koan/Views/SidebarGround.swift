#if os(macOS)
import AppKit
import SwiftUI

/// The sidebar column's AppKit chrome, put on the theme's terms.
///
/// macOS 26 wraps the sidebar column in an `NSGlassEffectView`. Even set to
/// clear, the glass lightens what it holds, so over the page's ground the
/// sidebar read as a lighter panel. SwiftUI offers no way to turn it off, nor
/// the source list's rounded selection, so this finds both from inside the
/// column.
///
/// In the theme, the column's content is lifted out of the glass into the
/// same place beside it, and the glass is hidden: the sidebar shows the
/// ground behind it, the wash or the column's flat `bg`, exactly as the page
/// does. The outline view keeps its selection — what VoiceOver announces and
/// the arrow keys move — but does not draw it; the row draws the theme's mark.
/// The theme is read at launch, so the platform's look is never touched.
struct SidebarGround: NSViewRepresentable {
    let themed: Bool

    func makeNSView(context: Context) -> Finder { Finder() }

    func updateNSView(_ view: Finder, context: Context) {
        view.themed = themed
        view.apply()
    }

    final class Finder: NSView {
        var themed = false
        private weak var glass: NSGlassEffectView?
        private var lifting = false

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            apply()
        }

        override func hitTest(_ point: NSPoint) -> NSView? { nil }

        func apply() {
            guard themed, !lifting, window != nil else { return }
            if glass == nil { lift() }
            guard let glass else { return }
            // AppKit shows the glass again when the column is collapsed and
            // brought back.
            glass.isHidden = true
            guard let column = glass.superview else { return }
            Self.walk(column) { view in
                if let outline = view as? NSOutlineView, outline.selectionHighlightStyle != .none {
                    outline.selectionHighlightStyle = .none
                }
            }
        }

        private func lift() {
            var view = superview
            while let candidate = view, !(candidate is NSGlassEffectView) { view = candidate.superview }
            guard let glass = view as? NSGlassEffectView,
                  let content = glass.contentView,
                  let column = glass.superview else { return }
            lifting = true
            defer { lifting = false }
            glass.contentView = nil
            column.addSubview(content, positioned: .above, relativeTo: glass)
            content.translatesAutoresizingMaskIntoConstraints = false
            NSLayoutConstraint.activate([
                content.leadingAnchor.constraint(equalTo: glass.leadingAnchor),
                content.trailingAnchor.constraint(equalTo: glass.trailingAnchor),
                content.topAnchor.constraint(equalTo: glass.topAnchor),
                content.bottomAnchor.constraint(equalTo: glass.bottomAnchor),
            ])
            self.glass = glass
        }

        private static func walk(_ view: NSView, _ visit: (NSView) -> Void) {
            visit(view)
            view.subviews.forEach { walk($0, visit) }
        }
    }
}
#endif
