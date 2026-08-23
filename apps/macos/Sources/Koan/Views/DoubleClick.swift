import AppKit
import SwiftUI

/// Double-click on a List row, done through AppKit.
///
/// Two approaches failed before this one. `.onTapGesture(count: 2)` makes
/// SwiftUI hold the first click back to see whether a second follows, which
/// leaves single-click selection feeling dead. A local `NSEvent` monitor never
/// sees the clicks at all — the list consumes them first.
///
/// So: attach one `NSClickGestureRecognizer` to the table itself. AppKit's own
/// click handling runs untouched, so selection behaves exactly as it does in
/// any other table, and only the second click reaches us.
///
/// Placed inside a *row* rather than on the List. A `.background` on the List
/// lands outside the list's own view hierarchy — its ancestors are the split
/// view column — so walking up from there never finds the table.
struct DoubleClickCatcher: NSViewRepresentable {
    let action: () -> Void

    func makeNSView(context: Context) -> NSView { PassthroughView() }

    func updateNSView(_ view: NSView, context: Context) {
        let action = self.action
        Task { @MainActor in
            guard let table = Self.enclosingTable(of: view) else { return }
            // Every row installs the same action, so last writer wins and the
            // closure stays current as the view is rebuilt.
            Registry.shared.register(action, for: table)
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

    /// Keeps one recogniser per table and the action it should run.
    @MainActor
    private final class Registry {
        static let shared = Registry()
        private var actions: [ObjectIdentifier: () -> Void] = [:]

        func register(_ action: @escaping () -> Void, for table: NSTableView) {
            let key = ObjectIdentifier(table)
            let isNew = actions[key] == nil
            actions[key] = action
            guard isNew else { return }

            let recogniser = NSClickGestureRecognizer(target: self, action: #selector(fire(_:)))
            recogniser.numberOfClicksRequired = 2
            // Without this the recogniser holds back the first click, which is
            // the whole problem we are avoiding.
            recogniser.delaysPrimaryMouseButtonEvents = false
            table.addGestureRecognizer(recogniser)
        }

        @objc private func fire(_ sender: NSClickGestureRecognizer) {
            guard let table = sender.view as? NSTableView else { return }
            actions[ObjectIdentifier(table)]?()
        }
    }
}

/// Invisible and untouchable: it exists only to find the table it sits in.
private final class PassthroughView: NSView {
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

extension View {
    /// Run `action` when a row in the enclosing List is double-clicked.
    /// Apply to a row, not to the List.
    func onRowDoubleClick(perform action: @escaping () -> Void) -> some View {
        background(DoubleClickCatcher(action: action))
    }
}
