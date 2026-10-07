import AVFAudio
import Foundation
import KoanFFI
#if !os(tvOS)
import UserNotifications
#endif

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
    /// The name of the port audio now goes to — "AirPods Pro", "Headphones",
    /// "Speaker" — which DSP profiles are chosen by. Told on activation and on
    /// every change.
    var onRoute: ((String) -> Void)? {
        didSet { reportRoute() }
    }

    private func reportRoute() {
        if let port = AVAudioSession.sharedInstance().currentRoute.outputs.first {
            onRoute?(port.portName)
        }
    }
    /// Where interruption events are written, so a failure to resume on a
    /// real phone can be read back afterwards.
    var note: ((String) -> Void)?

    /// Held apart from the actor so `deinit`, which is nonisolated, can still
    /// hand them back.
    private final class Tokens: @unchecked Sendable {
        var held: [NSObjectProtocol] = []
    }
    private let observers = Tokens()

    /// Category and buffer size, and the notifications. Not activation: an
    /// active `.playback` session stops whatever else is playing, so it is
    /// activated only when koan plays — by the engine, through `Host`.
    func prepare() {
        configure()
        observe()
    }

    /// Category and buffer size: also what a media services reset undoes.
    private func configure() {
        let session = AVAudioSession.sharedInstance()
        do {
            // `.playback` is what keeps producing audio with the screen locked
            // and the app backgrounded — paired with UIBackgroundModes: audio in
            // the bundle, without which the process is suspended and the audio
            // thread with it.
            try session.setCategory(.playback, mode: .default, options: [])
            // Larger than the default of a few milliseconds, so the render
            // thread wakes a twentieth as often. Latency is no cost to a music
            // player — the ring holds seconds, and pause fades out anyway —
            // and each wake is the CPU leaving idle.
            try session.setPreferredIOBufferDuration(0.093)
        } catch {
            NSLog("koan: audio session refused its configuration: \(error)")
        }
    }

    /// What the engine calls, on the player thread, the moment before its
    /// output starts and once it has been stopped for a while. Exclusive when
    /// active, as playing bit-for-bit wants; on release the app koan
    /// interrupted is told it may resume.
    final class Host: AudioSessionHost, @unchecked Sendable {
        /// Where activation failures are written.
        let note: @Sendable (String) -> Void

        init(note: @escaping @Sendable (String) -> Void) {
            self.note = note
        }

        /// `sampleRate` is what the output was built for, asked for as the
        /// preferred rate: a USB DAC that supports it is switched to it, while
        /// the speaker, Bluetooth and AirPlay keep their own and RemoteIO
        /// resamples. Answers the rate the hardware runs at, so the format
        /// badge shows which happened; nil when iOS refuses activation —
        /// during a call, or from the background with no remote command
        /// behind it — and the engine then does not start.
        func activate(sampleRate: Double) -> Double? {
            let session = AVAudioSession.sharedInstance()
            prefer(sampleRate)
            do {
                try session.setActive(true)
                return session.sampleRate
            } catch {
                note("audio session refused activation: \(error)")
                if (error as NSError).code == AVAudioSession.ErrorCode.cannotInterruptOthers.rawValue {
                    askToBeTapped()
                }
                return nil
            }
        }

        /// iOS will not give the session to an app in the background while
        /// another app plays, so music asked for from another device cannot
        /// start here. Say so where the person will see it, as a push does for a
        /// suspended phone: tapping opens the app, which may then play.
        private func askToBeTapped() {
            #if !os(tvOS)
            let content = UNMutableNotificationContent()
            content.title = "kōan could not start playing"
            content.body = "iOS needs kōan open to take over the audio. Tap to play."
            content.userInfo = ["koan": ["type": "resume"]]
            UNUserNotificationCenter.current().add(
                UNNotificationRequest(identifier: "koan.refused-activation", content: content, trigger: nil)
            )
            #endif
        }

        /// A route change: the new route asked for the output's rate, and
        /// the rate it runs at answered. Not an activation, which would take
        /// the session back from an app that interrupted koan.
        func follow(sampleRate: Double) -> Double {
            prefer(sampleRate)
            return AVAudioSession.sharedInstance().sampleRate
        }

        /// A preference already set is left alone, so asking again on a
        /// route change cannot itself change the route.
        private func prefer(_ sampleRate: Double) {
            let session = AVAudioSession.sharedInstance()
            guard session.preferredSampleRate != sampleRate else { return }
            do {
                try session.setPreferredSampleRate(sampleRate)
            } catch {
                note("audio session refused \(sampleRate) Hz: \(error)")
            }
        }

        func release() {
            do {
                try AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
            } catch {
                note("audio session refused deactivation: \(error)")
            }
        }
    }

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
                #if os(tvOS)
                let reason: UInt = 0
                #else
                let reason = note.userInfo?[AVAudioSessionInterruptionReasonKey] as? UInt ?? 0
                #endif
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

    /// An interruption began and nothing has ended it yet. The session is
    /// inactive until something reactivates it, and until then every play
    /// goes nowhere.
    private var interrupted = false
    /// When the route last went away under us, so an interruption that comes
    /// with it is not taken as one to play on through: headphones pulled out
    /// must stay paused.
    private var routeLostAt: Date?

    private func handleInterruption(
        _ type: AVAudioSession.InterruptionType, options: UInt, reason: UInt
    ) {
        note?("interruption \(type == .began ? "began" : "ended") options=\(options) reason=\(reason)")
        switch type {
        case .began:
            interrupted = true
            onInterrupted?()
            // iOS ends most interruptions with an "ended". One for a route
            // that went away never gets one (a voice assistant handing the
            // speaker back does this), and the session stays down with it. Take
            // the session back once the route has settled.
            if AVAudioSession.InterruptionReason(rawValue: reason) == .routeDisconnected {
                Task { [weak self] in
                    try? await Task.sleep(for: .seconds(1))
                    guard let self, self.interrupted else { return }
                    let lostRoute = self.routeLostAt.map { Date().timeIntervalSince($0) < 3 } ?? false
                    self.end(resume: !lostRoute)
                }
            }
        case .ended:
            let resume = AVAudioSession.InterruptionOptions(rawValue: options)
                .contains(.shouldResume)
            end(resume: resume)
        @unknown default:
            break
        }
    }

    /// The app is in front again. An interruption nothing ended leaves the
    /// output stopped by iOS; have it rebuilt, so pressing play plays. The
    /// session itself is left alone until then: whatever interrupted koan may
    /// still be playing. Resuming is the person's to do.
    func recoverIfInterrupted() {
        guard interrupted else { return }
        note?("session still interrupted on returning; rebuilding the output")
        end(resume: false)
    }

    /// Have the output rebuilt: the unit iOS stopped for the interruption will
    /// not start again on its own. The session is activated by the engine as
    /// the rebuilt output starts, and only if it does.
    private func end(resume: Bool) {
        interrupted = false
        onInterruptionEnded?(resume)
    }

    private func handleRouteChange(_ reason: AVAudioSession.RouteChangeReason) {
        let route = AVAudioSession.sharedInstance().currentRoute.outputs
            .map { $0.portType.rawValue }.joined(separator: ",")
        note?("route change reason=\(reason.rawValue) now=\(route)")
        reportRoute()
        // The only reason that must pause. The others — a better route
        // appearing, a category change — are not the user walking away.
        if reason == .oldDeviceUnavailable {
            routeLostAt = Date()
            onRouteLost?()
        }
    }

    deinit {
        let centre = NotificationCenter.default
        for observer in observers.held { centre.removeObserver(observer) }
    }
}
