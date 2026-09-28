import AVFAudio
import UIKit

/// Keeps a backgrounded, paused app running, so the server it is linked to can
/// still start music on it.
///
/// iOS suspends an app that is in the background and not playing audio, and a
/// suspended app's link to the server dies with it. Playing silence counts as
/// playing: the app stays alive and the link stays up. It costs the audio
/// hardware staying awake, so on battery it lasts `minutes` after playback
/// stops, and on the charger it lasts as long as the app is in the background.
/// A push notification is the proper way to wake an app, and needs a paid
/// developer account.
@MainActor
final class Keepalive {
    /// UserDefaults keys the Settings pane writes.
    static let enabledKey = "stayReachable"
    static let minutesKey = "stayReachableMinutes"
    static let defaultMinutes = 30

    private var player: AVAudioPlayer?
    private var background = false
    private var playing = false
    private var stoppedAt = Date()
    private var expiry: Task<Void, Never>?
    private let observers = Observers()

    private final class Observers: @unchecked Sendable {
        var held: [NSObjectProtocol] = []
    }

    init() {
        UIDevice.current.isBatteryMonitoringEnabled = true
        observers.held.append(
            NotificationCenter.default.addObserver(
                forName: UIDevice.batteryStateDidChangeNotification, object: nil, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated { self?.update() }
            }
        )
        // The Settings pane writes the toggle and the period straight to
        // defaults.
        observers.held.append(
            NotificationCenter.default.addObserver(
                forName: UserDefaults.didChangeNotification, object: nil, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated { self?.update() }
            }
        )
    }

    deinit {
        for observer in observers.held { NotificationCenter.default.removeObserver(observer) }
    }

    func setBackground(_ background: Bool) {
        self.background = background
        update()
    }

    func setPlaying(_ playing: Bool) {
        if self.playing && !playing { stoppedAt = Date() }
        self.playing = playing
        update()
    }

    /// Re-read the settings; call after they change.
    func update() {
        let defaults = UserDefaults.standard
        let enabled = defaults.object(forKey: Self.enabledKey) as? Bool ?? true
        let minutes = defaults.object(forKey: Self.minutesKey) as? Int ?? Self.defaultMinutes
        let charging = [.charging, .full].contains(UIDevice.current.batteryState)
        let deadline = stoppedAt.addingTimeInterval(TimeInterval(minutes * 60))

        expiry?.cancel()
        let wanted = enabled && background && !playing
        guard wanted, charging || minutes == 0 || Date() < deadline else {
            stop()
            return
        }
        start()
        if !charging, minutes > 0 {
            expiry = Task { [weak self] in
                try? await Task.sleep(for: .seconds(deadline.timeIntervalSinceNow))
                guard !Task.isCancelled else { return }
                self?.update()
            }
        }
    }

    private func start() {
        guard player == nil else { return }
        do {
            let silence = try AVAudioPlayer(data: Self.silentWav)
            silence.numberOfLoops = -1
            silence.volume = 0
            silence.play()
            player = silence
        } catch {
            NSLog("koan: keepalive could not start: \(error)")
        }
    }

    private func stop() {
        player?.stop()
        player = nil
    }

    /// One second of 8 kHz mono 16-bit silence, as a WAV file.
    private static let silentWav: Data = {
        let rate: UInt32 = 8000
        let samples = Data(count: Int(rate) * 2)
        var data = Data()
        func append<T: FixedWidthInteger>(_ value: T) {
            withUnsafeBytes(of: value.littleEndian) { data.append(contentsOf: $0) }
        }
        data.append(contentsOf: Array("RIFF".utf8))
        append(UInt32(36 + samples.count))
        data.append(contentsOf: Array("WAVEfmt ".utf8))
        append(UInt32(16))
        append(UInt16(1))  // PCM
        append(UInt16(1))  // mono
        append(rate)
        append(rate * 2)  // byte rate
        append(UInt16(2))  // block align
        append(UInt16(16))  // bits per sample
        data.append(contentsOf: Array("data".utf8))
        append(UInt32(samples.count))
        data.append(samples)
        return data
    }()
}
