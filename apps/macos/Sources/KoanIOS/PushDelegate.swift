import KoanFFI
import UIKit
import UserNotifications

/// Apple's push service: how a koan server reaches this app once iOS has
/// suspended it and its link has gone.
///
/// Two kinds arrive. A background push wakes the app for half a minute: it
/// links, and whatever waited in the server's outbox (a sync, an eviction)
/// comes down the link. A notification asks for music; iOS will not let a
/// suspended app start playing on its own, so it waits for a tap, and the
/// command it carries runs then.
@MainActor
final class PushDelegate: NSObject, UIApplicationDelegate, UNUserNotificationCenterDelegate {
    /// Set once the engine is up. A token or a tapped notification that
    /// arrives before then waits for it: a tap can be what launched the app.
    static var engine: KoanEngine? {
        didSet { flush() }
    }

    private static var token: String?
    private static var waiting: [String] = []

    /// Which of Apple's two gateways issued the token. A development build is
    /// signed for the sandbox; TestFlight and the App Store for production.
    private static let sandbox: Bool = {
        #if DEBUG
        true
        #else
        false
        #endif
    }()

    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        UNUserNotificationCenter.current().delegate = self
        // A token needs no permission; showing a notification does, and is
        // asked for once there is a server to send one.
        application.registerForRemoteNotifications()
        return true
    }

    func application(_: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        Self.token = deviceToken.map { String(format: "%02x", $0) }.joined()
        NSLog("koan: push token issued (\(Self.sandbox ? "sandbox" : "production"))")
        Self.flush()
    }

    func application(_: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {
        NSLog("koan: push registration failed: \(error)")
    }

    /// A background push. The link does the work: what waits in the outbox
    /// comes down it once connected. This keeps the app awake long enough for
    /// the link to connect and take it. Music is never among it: iOS does not
    /// let an app it woke start audio, so the server sends that as a
    /// notification instead.
    func application(
        _: UIApplication,
        didReceiveRemoteNotification userInfo: [AnyHashable: Any]
    ) async -> UIBackgroundFetchResult {
        Self.engine?.logNote(message: "push: woken")
        Self.engine?.linkNudge()
        try? await Task.sleep(for: .seconds(20))
        return .newData
    }

    /// A notification tapped: run what it asked for. Nonisolated, and the
    /// command taken out as a string before crossing to the main actor: the
    /// notification objects themselves are not `Sendable`.
    nonisolated func userNotificationCenter(
        _: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        let command = Self.command(in: response.notification.request.content.userInfo)
        Task { @MainActor in
            if let command { Self.run(command) }
        }
        completionHandler()
    }

    /// One that arrives with the app open: the server thought it suspended.
    /// Shown, so the person can still take it up.
    nonisolated func userNotificationCenter(
        _: UNUserNotificationCenter,
        willPresent _: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        completionHandler([.banner, .sound])
    }

    /// Ask to show notifications, if a server is signed in to send them.
    /// Asked once; iOS remembers the answer.
    static func requestAlertsIfSignedIn() {
        guard let engine else { return }
        Task {
            let settings = await engine.settings()
            guard settings.remoteSignedIn else { return }
            _ = try? await UNUserNotificationCenter.current()
                .requestAuthorization(options: [.alert, .sound])
        }
    }

    private nonisolated static func command(in userInfo: [AnyHashable: Any]) -> String? {
        guard let koan = userInfo["koan"],
              let data = try? JSONSerialization.data(withJSONObject: koan)
        else { return nil }
        return String(data: data, encoding: .utf8)
    }

    private static func run(_ command: String) {
        guard let engine else {
            waiting.append(command)
            return
        }
        Task { try? await engine.runPushedCommand(command: command) }
    }

    private static func flush() {
        guard let engine else { return }
        if let token {
            engine.setPushToken(token: token, sandbox: sandbox)
            NSLog("koan: push token handed to the link")
        }
        let commands = waiting
        waiting = []
        for command in commands {
            run(command)
        }
    }
}
