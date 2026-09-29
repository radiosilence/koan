import Foundation
import UserNotifications

/// Attaches the album cover to a "Play on this iPhone" notification before iOS
/// shows it. The server puts a short-lived, self-authorising link to the cover
/// under `image`, so nothing here needs the app's sign-in. Anything that goes
/// wrong shows the notification as it came, without art.
final class NotificationService: UNNotificationServiceExtension, @unchecked Sendable {
    private let lock = NSLock()
    private var pending: (content: UNMutableNotificationContent, deliver: (UNNotificationContent) -> Void)?

    override func didReceive(
        _ request: UNNotificationRequest,
        withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void
    ) {
        guard let content = request.content.mutableCopy() as? UNMutableNotificationContent else {
            contentHandler(request.content)
            return
        }
        lock.withLock { pending = (content, contentHandler) }
        guard let link = content.userInfo["image"] as? String, let url = URL(string: link) else {
            finish()
            return
        }
        URLSession.shared.downloadTask(with: url) { [self] file, response, _ in
            guard let file, (response as? HTTPURLResponse)?.statusCode == 200 else {
                finish()
                return
            }
            // The download is deleted when this returns, and the attachment's
            // type is read from its extension.
            let cover = FileManager.default.temporaryDirectory
                .appendingPathComponent(UUID().uuidString)
                .appendingPathExtension("jpg")
            try? FileManager.default.moveItem(at: file, to: cover)
            finish(attaching: cover)
        }.resume()
    }

    override func serviceExtensionTimeWillExpire() {
        finish()
    }

    /// Deliver once, whichever of the download and the deadline comes first.
    private func finish(attaching file: URL? = nil) {
        guard let (content, deliver) = lock.withLock({ () -> (UNMutableNotificationContent, (UNNotificationContent) -> Void)? in
            defer { pending = nil }
            return pending
        }) else { return }
        if let file, let cover = try? UNNotificationAttachment(identifier: "cover", url: file) {
            content.attachments = [cover]
        }
        deliver(content)
    }
}
