import AVFAudio
import Foundation

/// The half of iOS audio that the engine cannot own.
///
/// `koan-core` opens a RemoteIO unit and drains the ring buffer into it, which
/// is all it does on macOS too. Everything around that is the session's, and the
/// session belongs to the app: it needs a run loop for the notifications and an
/// app lifecycle to be suspended against.
///
/// Three things have no macOS counterpart at all and are the reason this exists:
/// a category, or the unit produces nothing; interruption handling, or a phone
/// call leaves playback stopped with the UI insisting otherwise; and route
/// change handling, because pulling headphones out must pause rather than
/// announce the record to the room.
@MainActor
final class AudioSession {
    /// Told when the system takes the audio away and when it gives it back.
    /// Wired to the player rather than acting on its own — what "resume" means
    /// is the player's business.
    var onInterrupted: (() -> Void)?
    /// The interruption is over and the session active again; the output
    /// needs building again. `shouldResume` is the system's word on whether
    /// playing on is expected: set after a call or Siri, not when the user
    /// has started something else.
    var onInterruptionEnded: ((_ shouldResume: Bool) -> Void)?
    /// The route went away underneath us — headphones unplugged, a dock removed.
    var onRouteLost: (() -> Void)?
    /// Where interruption events are written, so a failure to resume on a
    /// real phone can be read back afterwards.
    var note: ((String) -> Void)?

    /// Held apart from the actor so `deinit`, which is nonisolated, can still
    /// hand them back.
    private final class Tokens: @unchecked Sendable {
        var held: [NSObjectProtocol] = []
    }
    private let observers = Tokens()

    func activate(preferredSampleRate: Double? = nil) {
        configure(preferredSampleRate: preferredSampleRate)
        observe()
    }

    /// Category, buffer size and activation: also what a media services
    /// reset undoes.
    private func configure(preferredSampleRate: Double? = nil) {
        let session = AVAudioSession.sharedInstance()
        do {
            // `.playback` is what keeps producing audio with the screen locked
            // and the app backgrounded — paired with UIBackgroundModes: audio in
            // the bundle, without which the process is suspended and the audio
            // thread with it.
            try session.setCategory(.playback, mode: .default, options: [])
            if let preferredSampleRate {
                // A request, not an instruction. iOS may answer with something
                // else, and everything crosses the system mixer regardless —
                // which is why koan makes no bit-perfect claim here.
                try session.setPreferredSampleRate(preferredSampleRate)
            }
            // Larger than the default of a few milliseconds, so the render
            // thread wakes a twentieth as often. Latency is no cost to a music
            // player — the ring holds seconds, and pause fades out anyway —
            // and each wake is the CPU leaving idle.
            try session.setPreferredIOBufferDuration(0.093)
            try session.setActive(true)
        } catch {
            NSLog("koan: audio session refused activation: \(error)")
        }
    }

    /// What the session actually settled on, as against what was asked for.
    var sampleRate: Double { AVAudioSession.sharedInstance().sampleRate }

    private func observe() {
        let centre = NotificationCenter.default
        // A `Notification` is not Sendable, so what crosses back to the actor is
        // the two numbers read out of it here rather than the notification.
        observers.held.append(
            centre.addObserver(
                forName: AVAudioSession.interruptionNotification,
                object: nil, queue: .main
            ) { [weak self] note in
                let raw = note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt
                let options = note.userInfo?[AVAudioSessionInterruptionOptionKey] as? UInt ?? 0
                let reason = note.userInfo?[AVAudioSessionInterruptionReasonKey] as? UInt ?? 0
                guard let raw, let type = AVAudioSession.InterruptionType(rawValue: raw) else {
                    return
                }
                MainActor.assumeIsolated {
                    self?.handleInterruption(type, options: options, reason: reason)
                }
            }
        )
        // A reset of the system's media services invalidates every audio
        // object the app holds: set the session up again and rebuild.
        observers.held.append(
            centre.addObserver(
                forName: AVAudioSession.mediaServicesWereResetNotification,
                object: nil, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated {
                    self?.configure()
                    self?.onInterruptionEnded?(false)
                }
            }
        )
        observers.held.append(
            centre.addObserver(
                forName: AVAudioSession.routeChangeNotification,
                object: nil, queue: .main
            ) { [weak self] note in
                let raw = note.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt
                guard let raw, let reason = AVAudioSession.RouteChangeReason(rawValue: raw) else {
                    return
                }
                MainActor.assumeIsolated { self?.handleRouteChange(reason) }
            }
        )
    }

    private func handleInterruption(
        _ type: AVAudioSession.InterruptionType, options: UInt, reason: UInt
    ) {
        note?("interruption \(type == .began ? "began" : "ended") options=\(options) reason=\(reason)")
        switch type {
        case .began:
            onInterrupted?()
        case .ended:
            // The output unit iOS stopped for the interruption will not start
            // again, so it is rebuilt whether or not playback resumes.
            do {
                try AVAudioSession.sharedInstance().setActive(true)
            } catch {
                note?("could not reactivate the session: \(error)")
            }
            let resume = AVAudioSession.InterruptionOptions(rawValue: options)
                .contains(.shouldResume)
            onInterruptionEnded?(resume)
        @unknown default:
            break
        }
    }

    private func handleRouteChange(_ reason: AVAudioSession.RouteChangeReason) {
        // The only reason that must pause. The others — a better route
        // appearing, a category change — are not the user walking away.
        if reason == .oldDeviceUnavailable { onRouteLost?() }
    }

    deinit {
        let centre = NotificationCenter.default
        for observer in observers.held { centre.removeObserver(observer) }
    }
}
