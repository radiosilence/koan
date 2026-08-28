import AppKit
import Foundation
import KoanFFI

/// The audio behind the playing indicators.
///
/// One subscription for the whole app: the queue and a track list can both
/// have a current row on screen, and they are watching the same music. Nothing
/// here is polled and nothing is timed — the analyser publishes a frame and
/// this wakes on it. When the play head stops and the bars have fallen, it
/// publishes nothing, so this loop is asleep and costs exactly nothing until
/// there is music again.
///
/// The rate is the refresh rate of the display the window is on, which is
/// something only the window knows: koan sets it here and follows it across
/// screens.
@MainActor
@Observable
final class PlayingLevels {
    private let engine: KoanEngine

    /// How high each bar stands, 0...1, low band to high. The spectrum in
    /// three columns: what the analyser says is coming out of the speakers,
    /// and nothing else. Silence is zero and reads flat.
    ///
    /// Observed, and mutated once per published frame — which is the only
    /// reason an indicator ever redraws.
    private(set) var bands = [0.0, 0.0, 0.0]

    @ObservationIgnored private var stamp = Date().timeIntervalSinceReferenceDate
    @ObservationIgnored private var follow: Task<Void, Never>?

    /// The loudest each band has been lately. Each band is judged against its
    /// own recent range rather than against full scale, which is what stops a
    /// track mastered quiet getting a limper indicator than a loud one — and
    /// incidentally undoes the analyser's A-weighting tilt, which otherwise
    /// leaves the bass bar permanently the sluggish one.
    @ObservationIgnored private var ceiling = [quietest, quietest, quietest]

    /// A band below this is room tone, and never sets a ceiling. It is also
    /// what a silent passage is measured against, so silence stays flat rather
    /// than being normalised back up into a dance.
    private static let quietest = 0.12
    /// How long a band takes to forget a loud passage.
    private static let forget = 4.0

    init(engine: KoanEngine) {
        self.engine = engine
        let stream = engine.vizStream()
        follow = Task { [weak self] in
            while let levels = await stream.next() {
                guard let self else { return }
                take(levels)
            }
        }
        // The rate the analyser should run at is the refresh rate of the
        // display it is drawn on, which changes when the window is dragged to
        // another screen and when a screen is reconfigured under it.
        for name in [
            NSWindow.didChangeScreenNotification,
            NSWindow.didBecomeKeyNotification,
            NSApplication.didChangeScreenParametersNotification,
        ] {
            NotificationCenter.default.addObserver(
                forName: name, object: nil, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated { self?.matchDisplay() }
            }
        }
        matchDisplay()
    }

    deinit { follow?.cancel() }

    /// A frame, as it arrives. The only smoothing left here is the ceiling
    /// each band is measured against — the fall is the analyser's, which is
    /// also what decays the bars to flat when the music stops.
    private func take(_ levels: VizLevels) {
        let now = Date().timeIntervalSinceReferenceDate
        // Clamped: a machine that slept owes the bars nothing.
        let elapsed = min(max(now - stamp, 0), 0.25)
        stamp = now
        let hold = pow(0.5, elapsed / Self.forget)

        let heard = [Double(levels.low), Double(levels.mid), Double(levels.high)]
        var next = bands
        for band in heard.indices {
            ceiling[band] = max(heard[band], max(ceiling[band] * hold, Self.quietest))
            next[band] = min(heard[band] / ceiling[band], 1)
        }
        bands = next
    }

    private func matchDisplay() {
        let main = NSApp.windows.first { $0.identifier?.rawValue == MainWindow.id }
        let screen = main?.screen ?? NSApp.keyWindow?.screen ?? NSScreen.main
        engine.setVizFps(fps: UInt8(clamping: screen?.maximumFramesPerSecond ?? 60))
    }
}
