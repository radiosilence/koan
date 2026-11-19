import KoanFFI
import UIKit

/// Saves the session, and the queue kept on the server, when the app goes to
/// the background. iOS ends a suspended app without telling it, so this is
/// where a phone saves what a quit saves on a desktop. Held as a background
/// task: the app is suspended soon after it leaves, and a save cut off there
/// is the session lost.
@MainActor
final class SaveOnLeaving {
    private var observer: NSObjectProtocol?

    init(engine: KoanEngine) {
        observer = NotificationCenter.default.addObserver(
            forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: .main
        ) { _ in
            MainActor.assumeIsolated {
                let held = HeldTask()
                Task {
                    try? await engine.saveSession()
                    await engine.saveServerQueueNow()
                    held.end()
                }
            }
        }
    }
}

@MainActor
private final class HeldTask {
    private var id = UIBackgroundTaskIdentifier.invalid

    init() {
        id = UIApplication.shared.beginBackgroundTask(withName: "koan save") { [weak self] in
            MainActor.assumeIsolated { self?.end() }
        }
    }

    func end() {
        guard id != .invalid else { return }
        UIApplication.shared.endBackgroundTask(id)
        id = .invalid
    }
}
