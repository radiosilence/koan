#if os(macOS)
import AppKit

/// Clears the band above the sidebar in full screen.
///
/// In full screen the toolbar moves into a window of its own, and AppKit backs
/// the sidebar's share of it with an opaque view, `NSOpaqueBackstop`: white in
/// light mode, black in dark, over a sidebar that otherwise shows the wash.
/// Nothing SwiftUI offers reaches that window, so the view is found by its
/// class name and hidden. If a later macOS renames it, this finds nothing and
/// the band comes back; nothing else changes.
@MainActor
enum FullScreenBackstop {
    private static var observer: NSObjectProtocol?

    static func install() {
        guard observer == nil else { return }
        observer = NotificationCenter.default.addObserver(
            forName: NSWindow.didEnterFullScreenNotification, object: nil, queue: .main
        ) { _ in
            MainActor.assumeIsolated {
                for window in NSApp.windows {
                    guard let root = window.contentView?.superview else { continue }
                    hide(in: root)
                }
            }
        }
    }

    private static func hide(in view: NSView) {
        if NSStringFromClass(type(of: view)) == "NSOpaqueBackstop" {
            view.isHidden = true
            return
        }
        for subview in view.subviews { hide(in: subview) }
    }
}
#endif
