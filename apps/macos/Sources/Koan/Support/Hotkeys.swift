import AppKit

/// A single-key shortcut, ported from the TUI.
///
/// The label is what the shortcuts sheet shows, so the keys and their
/// documentation cannot drift apart.
struct Hotkey {
    enum Group: String, CaseIterable {
        case playback = "Playback"
        case navigation = "Navigation"
        case view = "View"
    }

    let keys: [String]
    let label: String
    let group: Group
    let action: () -> Void
}

/// Bare-key shortcuts, live everywhere except where a key means something else.
///
/// These cannot be menu shortcuts. A modifier-less key equivalent is claimed by
/// the menu before the responder chain sees it, so `f` would favourite a track
/// instead of typing an f into the search field — which is why AppKit warns you
/// off declaring them. A local monitor can ask what has focus first, and that is
/// the whole point: the keys are live in the app and dead in any text field.
///
/// A focused List eats space for scrolling and letters for type-select, so the
/// monitor also wins the keys a menu would have lost anyway. The cost is that
/// type-select in lists is gone; koan's browsers filter through their own field.
@MainActor
final class Hotkeys {
    private var monitor: Any?
    private let bindings: [String: Hotkey]

    /// In table order, for the shortcuts sheet.
    let all: [Hotkey]

    init(_ all: [Hotkey]) {
        self.all = all
        self.bindings = Dictionary(
            uniqueKeysWithValues: all.flatMap { hotkey in hotkey.keys.map { ($0, hotkey) } }
        )
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self else { return event }
            return self.handle(event) ? nil : event
        }
    }

    /// The monitor outlives this object only if the app is tearing down anyway,
    /// so there is nothing to unwind from a nonisolated deinit.
    func stop() {
        if let monitor { NSEvent.removeMonitor(monitor) }
        monitor = nil
    }

    /// Returns true when the event has been consumed.
    private func handle(_ event: NSEvent) -> Bool {
        // Shift is part of the key here — `>` is one. Anything else means the
        // user is aiming at a menu shortcut.
        let modifiers = event.modifierFlags
            .intersection(.deviceIndependentFlagsMask)
            .subtracting(.shift)
        guard modifiers.isEmpty, let window = ownWindow else { return false }

        // Escape gets you back out of a search or filter field. AppKit only
        // clears the text; leaving focus behind means the next key you press is
        // still going into a field you thought you had left.
        if event.keyCode == 53, EditCommands.isEditingText {
            window.makeFirstResponder(nil)
            return true
        }

        guard !EditCommands.isEditingText else { return false }
        guard let key = event.charactersIgnoringModifiers, let hotkey = bindings[key] else {
            return false
        }
        hotkey.action()
        return true
    }

    /// The main window, or nil when focus is somewhere these keys have no
    /// business being — a sheet, the settings window, or the organize window,
    /// whose own controls are what a key should reach.
    private var ownWindow: NSWindow? {
        guard let window = NSApp.keyWindow else { return nil }
        guard !window.isSheet, window.attachedSheet == nil else { return nil }
        guard window.identifier?.rawValue != "com_apple_SwiftUI_Settings_window" else {
            return nil
        }
        // Organize is its own window with its own field and its own Esc. Same
        // reasoning as the settings window: its controls are what a key there
        // should reach.
        guard window.identifier?.rawValue != "organize" else { return nil }
        return window
    }
}

extension Hotkeys {
    /// The map, following the TUI's where the two apps do the same thing.
    ///
    /// Keys the TUI spends on its own modes — edit mode, the visualizer, the
    /// help overlay's own navigation — have nothing to point at here, and the
    /// ⌘ shortcuts in the menu bar cover what the menus already say.
    static func standard(
        player: PlayerModel,
        library: LibraryModel,
        ui: UIState
    ) -> Hotkeys {
        Hotkeys([
            Hotkey(keys: [" "], label: "Play / pause", group: .playback) {
                player.togglePlayPause()
            },
            Hotkey(keys: ["<"], label: "Previous track", group: .playback) {
                player.previous()
            },
            Hotkey(keys: [">", "n"], label: "Next track", group: .playback) {
                player.next()
            },
            Hotkey(keys: [","], label: "Back 10 seconds", group: .playback) {
                player.seek(bySeconds: -10)
            },
            Hotkey(keys: ["."], label: "Forward 10 seconds", group: .playback) {
                player.seek(bySeconds: 10)
            },
            Hotkey(keys: ["f"], label: "Favourite this track", group: .playback) {
                player.toggleFavouriteCurrent()
            },
            Hotkey(keys: ["R"], label: "Radio mode", group: .playback) {
                player.toggleRadio()
            },

            Hotkey(keys: ["p"], label: "Add music", group: .navigation) {
                ui.showingPicker = true
            },
            Hotkey(keys: ["/"], label: "Search library", group: .navigation) {
                NSLog("HOTKEY slash -> focusSearch")
                ui.focusSearch()
            },
            Hotkey(keys: ["l", "a"], label: "Albums", group: .navigation) {
                library.section = .albums
            },
            Hotkey(keys: ["r"], label: "Artists", group: .navigation) {
                library.section = .artists
            },
            Hotkey(keys: ["g"], label: "Top of the queue", group: .navigation) {
                library.section = .queue
                ui.jumpQueue(to: .top)
            },
            Hotkey(keys: ["G"], label: "End of the queue", group: .navigation) {
                library.section = .queue
                ui.jumpQueue(to: .bottom)
            },

            Hotkey(keys: ["L"], label: "Lyrics panel", group: .view) {
                ui.toggleLyrics()
            },
            Hotkey(keys: ["z"], label: "Zoom the cover", group: .view) {
                guard player.currentTrackId != nil else { return }
                ui.showingArtwork = true
            },
            Hotkey(keys: ["?"], label: "This list", group: .view) {
                ui.showingShortcuts = true
            },
        ])
    }
}
