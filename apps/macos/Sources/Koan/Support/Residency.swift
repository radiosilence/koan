#if os(macOS)
import AppKit
import KoanFFI
import Observation
import OSLog
import ServiceManagement

/// Whether kōan stays running with its window closed, and opens at login.
///
/// A Mac cannot be woken by a push the way a phone can: a quit kōan is out of
/// reach until someone opens it. Staying resident in the menu bar is what
/// keeps it controllable from other devices, and opening at login is what
/// brings it back after a restart.
///
/// While resident with no window open, kōan leaves the Dock and ⌘-Tab
/// (accessory activation policy) and returns to them when its window opens.
@MainActor
@Observable
final class Residency {
    /// `devices.keep_running`, as last read or set. Written through the
    /// settings window's model, which holds the rest of the settings and would
    /// otherwise write its own copy of this back over it.
    var keepRunning = false {
        didSet { if !keepRunning { showInDock() } }
    }

    /// Whether macOS will open kōan at login. Asked of the system, which owns
    /// it, rather than kept in the configuration: it can be turned off in
    /// System Settings ▸ General ▸ Login Items without kōan being told.
    private(set) var opensAtLogin = SMAppService.mainApp.status == .enabled
    /// Registered, but waiting for approval in System Settings.
    private(set) var loginNeedsApproval = SMAppService.mainApp.status == .requiresApproval
    private(set) var loginError: String?

    @ObservationIgnored private var observers: [NSObjectProtocol] = []
    @ObservationIgnored private let log = Logger(subsystem: "cc.blit.koan", category: "residency")

    init() {
        let centre = NotificationCenter.default
        observers.append(
            centre.addObserver(forName: NSWindow.willCloseNotification, object: nil, queue: .main) {
                [weak self] note in
                guard (note.object as? NSWindow)?.identifier?.rawValue == MainWindow.id else { return }
                MainActor.assumeIsolated { self?.mainWindowClosed() }
            })
        observers.append(
            centre.addObserver(forName: NSWindow.didBecomeKeyNotification, object: nil, queue: .main) {
                [weak self] note in
                guard (note.object as? NSWindow)?.identifier?.rawValue == MainWindow.id else { return }
                MainActor.assumeIsolated { self?.showInDock() }
            })
    }

    func load(engine: KoanEngine) async {
        keepRunning = await engine.settings().devicesKeepRunning
    }

    /// Re-read what the system says, for a settings pane coming to the front.
    func refreshLogin() {
        let status = SMAppService.mainApp.status
        opensAtLogin = status == .enabled
        loginNeedsApproval = status == .requiresApproval
    }

    func setOpensAtLogin(_ on: Bool) {
        do {
            if on {
                try SMAppService.mainApp.register()
            } else {
                try SMAppService.mainApp.unregister()
            }
            loginError = nil
        } catch {
            loginError = error.localizedDescription
            log.error("login item: \(on ? "register" : "unregister") failed: \(error)")
        }
        refreshLogin()
    }

    private func mainWindowClosed() {
        guard keepRunning else { return }
        log.info("window closed; staying in the menu bar")
        NSApp.setActivationPolicy(.accessory)
    }

    private func showInDock() {
        if NSApp.activationPolicy() != .regular {
            NSApp.setActivationPolicy(.regular)
        }
    }
}

/// Decides whether closing the last window quits kōan.
final class AppDelegate: NSObject, NSApplicationDelegate {
    @MainActor var residency: Residency?

    @MainActor
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        !(residency?.keepRunning ?? false)
    }
}
#endif
