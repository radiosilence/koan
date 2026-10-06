#if os(macOS)
import AppKit
import KoanFFI
import Observation
import OSLog
import ServiceManagement
import SwiftUI

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
    /// A run with no one at the screen: the evidence renderer
    /// (`KOAN_RENDER_EVIDENCE`). The engine runs; no window opens.
    nonisolated static var windowless: Bool {
        ProcessInfo.processInfo.environment["KOAN_RENDER_EVIDENCE"] != nil
    }

    /// `devices.keep_running`, as last read or set. Written through the
    /// settings window's model, which holds the rest of the settings and would
    /// otherwise write its own copy of this back over it.
    var keepRunning = false {
        didSet {
            // AppKit quits a windowless app on its own when it judges nobody
            // will notice, which a menu bar app that other devices control is
            // not. Off the menu bar, the app's own choice stands.
            ProcessInfo.processInfo.automaticTerminationSupportEnabled =
                keepRunning ? false : Self.automaticTermination
            if !keepRunning { showInDock() }
        }
    }
    private static let automaticTermination = ProcessInfo.processInfo.automaticTerminationSupportEnabled

    /// The main window should open as soon as something that can open it is
    /// on screen: the menu bar item, which has SwiftUI's `openWindow` and
    /// AppKit's delegate does not.
    var wantsWindow = false

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
                let window = note.object as? NSWindow
                MainActor.assumeIsolated {
                    guard window?.identifier?.rawValue == MainWindow.id else { return }
                    self?.mainWindowClosed()
                }
            })
        observers.append(
            centre.addObserver(forName: NSWindow.didBecomeKeyNotification, object: nil, queue: .main) {
                [weak self] note in
                let window = note.object as? NSWindow
                MainActor.assumeIsolated {
                    guard window?.identifier?.rawValue == MainWindow.id else { return }
                    self?.showInDock()
                }
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

    /// Open the main window, as the menu bar item's Open kōan does.
    func showWindow(with openWindow: OpenWindowAction) {
        wantsWindow = false
        NSApp.setActivationPolicy(.regular)
        openWindow(id: MainWindow.id)
        NSApp.activate()
    }

    static var mainWindowShown: Bool {
        NSApp.windows.contains { $0.identifier?.rawValue == MainWindow.id && $0.isVisible }
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

/// Starts the engine at launch, and decides whether closing the last window
/// quits kōan.
///
/// The engine is started here rather than by the main window because a window
/// closed to the menu bar is restored closed: a kōan opened at login would
/// otherwise have no engine, and so be out of reach, until someone opened it.
@MainActor
@Observable
final class AppDelegate: NSObject, NSApplicationDelegate {
    private(set) var state: AppState?
    private(set) var startupError: String?
    /// A link opened before the engine was up, handled once it is.
    @ObservationIgnored var pendingURL: URL?
    /// Opened by macOS at login rather than by someone: the one launch that
    /// starts in the menu bar without its window.
    @ObservationIgnored private var launchedAtLogin = false

    func applicationWillFinishLaunching(_ notification: Notification) {
        launchedAtLogin =
            NSAppleEventManager.shared().currentAppleEvent?
            .paramDescriptor(forKeyword: keyAELaunchedAsLogInItem) != nil
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        Task { await start() }
    }

    private func start() async {
        do {
            let created = try await AppState()
            await created.start()
            state = created
            if let dir = EvidenceRenderer.directory {
                await EvidenceRenderer.run(created, into: dir)
            }
            if let pendingURL {
                created.open(url: pendingURL)
                self.pendingURL = nil
            }
        } catch {
            startupError = String(describing: error)
        }
        // Restored with the window closed. Opened at login, a resident kōan
        // belongs in the menu bar only; opened by someone, they want it.
        guard !Residency.windowless,
              let residency = state?.residency, residency.keepRunning, !Residency.mainWindowShown
        else { return }
        if launchedAtLogin {
            NSApp.setActivationPolicy(.accessory)
        } else {
            residency.wantsWindow = true
        }
    }

    /// Opening kōan again from Finder, Spotlight or the Dock while it runs
    /// in the menu bar: the window it was opened for. Otherwise SwiftUI's own
    /// handling.
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows: Bool) -> Bool {
        guard let residency = state?.residency, residency.keepRunning, !Residency.mainWindowShown
        else { return true }
        residency.wantsWindow = true
        return false
    }

    /// Not before the setting is read: AppKit asks at launch, when state
    /// restoration has opened no window.
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        guard let state else { return false }
        return !state.residency.keepRunning
    }
}
#endif
