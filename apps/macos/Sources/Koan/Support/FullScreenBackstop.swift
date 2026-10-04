#if os(macOS)
import AppKit

/// Clears the band above the sidebar in full screen.
///
/// In full screen the toolbar moves into a window of its own, and AppKit backs
/// the sidebar's share of it with an opaque view, `NSOpaqueBackstop`: white in
/// light mode, black in dark, over a sidebar that otherwise shows the wash.
/// Nothing SwiftUI offers reaches that window, so the view is found by its
/// class name and hidden. If a later macOS renames either, this finds nothing
/// and the band comes back; nothing else changes.
///
/// Only that window is touched, never the main window's own hierarchy. AppKit
/// may build it after the window enters full screen and rebuilds its views when
/// the toolbar is revealed or the appearance changes, so while full screen
/// lasts it is swept each time it updates. Its view tree is a handful of views.
@MainActor
enum FullScreenBackstop {
    private static var observers: [NSObjectProtocol] = []
    private static var updates: NSObjectProtocol?

    static func install() {
        guard observers.isEmpty else { return }
        let centre = NotificationCenter.default
        observers.append(
            centre.addObserver(
                forName: NSWindow.didEnterFullScreenNotification, object: nil, queue: .main
            ) { _ in
                MainActor.assumeIsolated {
                    guard updates == nil else { return }
                    updates = centre.addObserver(
                        forName: NSWindow.didUpdateNotification, object: nil, queue: .main
                    ) { _ in
                        MainActor.assumeIsolated { NSApp.windows.forEach(sweep) }
                    }
                    NSApp.windows.forEach(sweep)
                }
            }
        )
        observers.append(
            centre.addObserver(
                forName: NSWindow.didExitFullScreenNotification, object: nil, queue: .main
            ) { _ in
                MainActor.assumeIsolated {
                    if let updates { centre.removeObserver(updates) }
                    updates = nil
                }
            }
        )
    }

    private static func sweep(_ window: NSWindow) {
        guard NSStringFromClass(type(of: window)).contains("ToolbarFullScreenWindow"),
              let root = window.contentView?.superview
        else { return }
        hide(in: root)
    }

    private static func hide(in view: NSView) {
        if NSStringFromClass(type(of: view)) == "NSOpaqueBackstop" {
            if !view.isHidden { view.isHidden = true }
            return
        }
        for subview in view.subviews { hide(in: subview) }
    }
}
#endif
