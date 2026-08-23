import AppKit
import SwiftUI

/// Double-click handling for a List, without a per-row gesture.
///
/// SwiftUI tap gestures on List rows compete with the table's own mouse
/// handling and swallow single clicks — the larger the gesture's hit area, the
/// more it eats. Installing one `NSClickGestureRecognizer` on the enclosing
/// table sidesteps that: AppKit delivers the first click to the table as
/// normal, so selection still works, and only the second click reaches us.
///
/// Acts on the selection rather than hit-testing a row, because by the time a
/// double-click completes the first click has already selected the row under
/// the pointer.
struct DoubleClickHandler: NSViewRepresentable {
    let action: () -> Void

    func makeNSView(context: Context) -> NSView {
        let view = PassthroughView()
        context.coordinator.action = action
        return view
    }

    func updateNSView(_ view: NSView, context: Context) {
        context.coordinator.action = action
        // The table doesn't exist yet while the representable is being made, so
        // attach on the next pass once the hierarchy is assembled.
        let coordinator = context.coordinator
        Task { @MainActor in coordinator.attach(from: view) }
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    @MainActor
    final class Coordinator: NSObject {
        var action: (() -> Void)?
        private weak var attachedTo: NSView?

        func attach(from view: NSView) {
            guard attachedTo == nil, let table = Self.enclosingTable(of: view) else { return }
            let recogniser = NSClickGestureRecognizer(target: self, action: #selector(fire))
            recogniser.numberOfClicksRequired = 2
            recogniser.delaysPrimaryMouseButtonEvents = false
            table.addGestureRecognizer(recogniser)
            attachedTo = table
        }

        @objc private func fire() { action?() }

        /// A `.background` view is a sibling of the table, not an ancestor of
        /// it, so walking up alone never finds it. Rise to the nearest scroll
        /// view — which the List does own — then search back down.
        private static func enclosingTable(of view: NSView) -> NSView? {
            var candidate: NSView? = view
            while let current = candidate {
                if let table = current as? NSTableView { return table }
                if let scroll = current as? NSScrollView {
                    if let table = scroll.documentView as? NSTableView { return table }
                    if let found = descendantTable(of: scroll) { return found }
                }
                if let found = descendantTable(of: current) { return found }
                candidate = current.superview
            }
            return nil
        }

        private static func descendantTable(of view: NSView) -> NSTableView? {
            for child in view.subviews {
                if let table = child as? NSTableView { return table }
                if let found = descendantTable(of: child) { return found }
            }
            return nil
        }
    }
}

/// Invisible and untouchable: it exists only to find the table it sits in.
private final class PassthroughView: NSView {
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}
