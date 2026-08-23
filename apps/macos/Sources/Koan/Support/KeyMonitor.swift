import AppKit

/// Space for play/pause, handled before anything else can eat it.
///
/// A focused list consumes space for its own scrolling and type-select, so a
/// menu shortcut loses to whatever has keyboard focus — which is why it worked
/// until the queue became a real List. A local event monitor sees the key
/// first, so the shortcut behaves the way it does in every other music player.
///
/// Deliberately ignores space while text is being edited: typing a space in the
/// search field must insert a space.
@MainActor
final class KeyMonitor {
    private var monitor: Any?
    private let onSpace: () -> Void

    init(onSpace: @escaping () -> Void) {
        self.onSpace = onSpace
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
        guard event.keyCode == 49 else { return false }  // space
        // Modifiers mean something else is intended.
        guard event.modifierFlags.intersection(.deviceIndependentFlagsMask).isEmpty else {
            return false
        }
        guard !EditCommands.isEditingText else { return false }
        onSpace()
        return true
    }

}
