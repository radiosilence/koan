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
/// column beside it, and the glass is hidden: the sidebar shows the ground
/// behind it, the wash or the column's flat `bg`, exactly as the page does.
/// AppKit may wrap the content in glass again, or show the glass, when the
/// column collapses, comes back or goes full screen, so every move of this
/// view, every resize of the split view and every unhiding of the glass looks
/// again. The outline view keeps its selection — what VoiceOver announces and
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
        private weak var outline: NSOutlineView?
        private var pins: [NSLayoutConstraint] = []
        private var unhiding: NSKeyValueObservation?
        private var resizes: NSObjectProtocol?
        private var lifting = false

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            if window == nil { stopWatching() }
            apply()
        }

        override func hitTest(_ point: NSPoint) -> NSView? { nil }

        func apply() {
            guard themed, !lifting, window != nil else { return }
            if let glass, glass.window == nil { self.glass = nil }
            // Inside glass: on the first pass, or wrapped again since.
            var view = superview
            while let candidate = view, !(candidate is NSGlassEffectView) { view = candidate.superview }
            if let wrapping = view as? NSGlassEffectView { lift(out: wrapping) }
            guard let glass else { return }
            if !glass.isHidden { glass.isHidden = true }
            watch(glass)
            if let outline, outline.window != nil, outline.selectionHighlightStyle == .none { return }
            guard let column = glass.superview else { return }
            Self.walk(column) { view in
                guard let found = view as? NSOutlineView else { return }
                found.selectionHighlightStyle = .none
                outline = found
            }
        }

        private func lift(out glass: NSGlassEffectView) {
            guard let content = glass.contentView, let column = glass.superview else { return }
            lifting = true
            defer { lifting = false }
            NSLayoutConstraint.deactivate(pins)
            glass.contentView = nil
            column.addSubview(content, positioned: .above, relativeTo: glass)
            content.translatesAutoresizingMaskIntoConstraints = false
            // To the column rather than the glass, which may go while the
            // content stays.
            pins = [
                content.leadingAnchor.constraint(equalTo: column.leadingAnchor),
                content.trailingAnchor.constraint(equalTo: column.trailingAnchor),
                content.topAnchor.constraint(equalTo: column.topAnchor),
                content.bottomAnchor.constraint(equalTo: column.bottomAnchor),
            ]
            NSLayoutConstraint.activate(pins)
            if self.glass !== glass { stopWatching() }
            self.glass = glass
        }

        private func watch(_ glass: NSGlassEffectView) {
            if unhiding == nil {
                unhiding = glass.observe(\.isHidden) { [weak self] _, _ in
                    Task { @MainActor in self?.apply() }
                }
            }
            if resizes == nil {
                var view = glass.superview
                while let candidate = view, !(candidate is NSSplitView) { view = candidate.superview }
                guard let split = view else { return }
                resizes = NotificationCenter.default.addObserver(
                    forName: NSSplitView.didResizeSubviewsNotification, object: split, queue: .main
                ) { [weak self] _ in
                    MainActor.assumeIsolated { self?.apply() }
                }
            }
        }

        private func stopWatching() {
            unhiding = nil
            if let resizes { NotificationCenter.default.removeObserver(resizes) }
            resizes = nil
        }

        private static func walk(_ view: NSView, _ visit: (NSView) -> Void) {
            visit(view)
            view.subviews.forEach { walk($0, visit) }
        }
    }
}
#endif
