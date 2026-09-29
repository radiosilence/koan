import ActivityKit
import AppIntents
import Foundation

/// The lock screen's view of another device this phone is controlling.
///
/// iOS gives the system's Now Playing controls only to the app producing the
/// audio, and here the audio is on another device. A Live Activity is what
/// iOS offers instead: the server pushes it each change the device reports,
/// so it stays right with the app suspended, and its buttons run
/// `RemoteControlIntent` without opening the app.
///
/// Compiled into both the app and the widget extension that draws it.
struct RemoteActivity: ActivityAttributes {
    /// What the server sends as `content-state`; see `push::ActivityState`.
    struct ContentState: Codable, Hashable {
        var device: String
        var linked: Bool
        var title: String?
        var artist: String?
        var album: String?
        var playing: Bool
        var positionMs: UInt64
        var durationMs: UInt64
        /// When `positionMs` was true, in Unix seconds.
        var at: Double
        /// The sleeve, a small JPEG; base64 in the server's pushes.
        var art: Data?

        /// When the track started, had it played without a pause since.
        var started: Date {
            Date(timeIntervalSince1970: at - Double(positionMs) / 1000)
        }

        var ends: Date {
            started.addingTimeInterval(Double(durationMs) / 1000)
        }
    }

    /// The device shown, which the buttons command.
    var deviceId: String
}

/// A button on the Live Activity: one link command to the device it shows.
/// Runs in the app's process, which iOS wakes for it.
struct RemoteControlIntent: LiveActivityIntent {
    static let title: LocalizedStringResource = "Control the device kōan is playing on"
    static let isDiscoverable = false

    @Parameter(title: "Device") var device: String
    /// The link command, as the link carries it: `{"type":"pause"}`.
    @Parameter(title: "Command") var command: String

    init() {}

    init(device: String, command: String) {
        self.device = device
        self.command = command
    }

    func perform() async throws -> some IntentResult {
        try await RemoteActivityCommands.shared.send(device: device, command: command)
        return .result()
    }
}

/// Where the intents' commands go. The app hands it the engine once it has
/// one; an intent that woke the app before then waits for it.
actor RemoteActivityCommands {
    static let shared = RemoteActivityCommands()

    typealias Handler = @Sendable (String, String) async throws -> Void
    private var handler: Handler?
    private var waiting: [CheckedContinuation<Handler, Never>] = []

    func set(_ handler: @escaping Handler) {
        self.handler = handler
        for w in waiting { w.resume(returning: handler) }
        waiting = []
    }

    func send(device: String, command: String) async throws {
        let handler = if let handler { handler } else {
            await withCheckedContinuation { waiting.append($0) }
        }
        try await handler(device, command)
    }
}
