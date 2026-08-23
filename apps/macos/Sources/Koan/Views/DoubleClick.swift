import AppKit
import SwiftUI

/// Double-click on a List row, done through AppKit, resolved to the row that
/// was actually clicked.
///
/// Earlier attempts failed in instructive ways. `.onTapGesture(count: 2)` makes
/// SwiftUI hold the first click back to see whether a second follows, leaving
/// single-click selection dead. A local `NSEvent` monitor never sees the clicks
/// at all — the list consumes them first. And registering one action per table
/// means the last row to render wins, which showed up as every double-click in
/// the artist list opening the same artist.
///
/// So: one `NSClickGestureRecognizer` per table with
/// `delaysPrimaryMouseButtonEvents = false`, so AppKit's own click handling runs
/// untouched, and the click location decides which row's action to run.
///
/// The catcher must be applied to a *row*. A `.background` on the List lands
/// outside the list's own hierarchy — its ancestors are the split-view column —
/// so walking up from there never finds the table.
struct DoubleClickCatcher: NSViewRepresentable {
    let action: () -> Void

    func makeNSView(context: Context) -> NSView {
        let view = PassthroughView()
        context.coordinator.view = view
        return view
    }

    func updateNSView(_ view: NSView, context: Context) {
        context.coordinator.action = action
        Task { @MainActor in
            Registry.shared.attach(context.coordinator, from: view)
        }
    }

    func makeCoordinator() -> RowEntry { RowEntry() }

    /// One row's claim on a double-click.
    @MainActor
    final class RowEntry {
        weak var view: NSView?
        var action: (() -> Void)?
    }

    @MainActor
    private final class Registry {
        static let shared = Registry()
        /// Rows per table. Weak views, so rows that scroll away drop out.
        private var rows: [ObjectIdentifier: [RowEntry]] = [:]
        private var attached: Set<ObjectIdentifier> = []

        func attach(_ entry: RowEntry, from view: NSView) {
            guard let table = Self.enclosingTable(of: view) else { return }
            let key = ObjectIdentifier(table)

            var known = rows[key] ?? []
            known.removeAll { $0.view == nil || $0 === entry }
            known.append(entry)
            rows[key] = known

            guard !attached.contains(key) else { return }
            attached.insert(key)
            let recogniser = NSClickGestureRecognizer(target: self, action: #selector(fire(_:)))
            recogniser.numberOfClicksRequired = 2
            // Without this the recogniser holds the first click back, which is
            // the problem we are avoiding.
            recogniser.delaysPrimaryMouseButtonEvents = false
            table.addGestureRecognizer(recogniser)
        }

        /// Run the action belonging to the row under the pointer.
        @objc private func fire(_ sender: NSClickGestureRecognizer) {
            guard let table = sender.view else { return }
            let point = sender.location(in: table)
            let candidates = rows[ObjectIdentifier(table)] ?? []
            for entry in candidates {
                guard let view = entry.view, view.window != nil else { continue }
                let frame = view.convert(view.bounds, to: table)
                if frame.contains(point) {
                    entry.action?()
                    return
                }
            }
        }

        private static func enclosingTable(of view: NSView) -> NSTableView? {
            var candidate: NSView? = view
            while let current = candidate {
                if let table = current as? NSTableView { return table }
                candidate = current.superview
            }
            return nil
        }
    }
}

/// Invisible and untouchable: it exists only to locate its row and its table.
private final class PassthroughView: NSView {
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

extension View {
    /// Run `action` when this row is double-clicked. Apply to a row, not a List.
    func onRowDoubleClick(perform action: @escaping () -> Void) -> some View {
        background(DoubleClickCatcher(action: action))
    }
}
