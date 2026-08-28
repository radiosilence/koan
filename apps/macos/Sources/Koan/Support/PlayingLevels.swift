import AppKit
import Foundation
import KoanFFI

/// The audio behind the playing indicators.
///
/// One source for the whole app: the queue and a track list can both have a
/// current row on screen, and they are watching the same music. There is no
/// timer here and nothing polls — an indicator asks for the bands as it draws
/// a frame, and the first to ask on a given frame is the one that reads the
/// analyser. Nothing on screen means nothing read, and the analyser stands
/// itself down a second after the last read.
///
/// The bands themselves are not observed. They move at the refresh rate of the
/// display and an observer would invalidate a view for each one; the
/// indicators redraw off their own display-linked timeline and take the value
/// as they go. `settled` is observed, because it is the one thing a view has
/// to be told rather than ask: it is what stops the timeline.
@MainActor
@Observable
final class PlayingLevels {
    private let engine: KoanEngine

    /// How high each bar stands, 0...1, low band to high. The spectrum in
    /// three columns: what the analyser says is coming out of the speakers,
    /// and nothing else. Silence is zero and reads flat.
    @ObservationIgnored private(set) var bands = [0.0, 0.0, 0.0]

    /// Whether the bars have come to rest with nothing playing. False while
    /// they still have somewhere to fall, which is what keeps the timeline
    /// ticking through the decay after a pause and stops it after.
    private(set) var settled = true

    @ObservationIgnored private var stamp = 0.0

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
    /// How long a band takes to fall away. Rises are not damped at all: this
    /// is the law the TUI's spectrum runs on — up on the frame it happens,
    /// down on a half-life so nothing snaps to zero between beats.
    private static let release = 0.15
    /// How long a band takes to forget a loud passage.
    private static let forget = 4.0
    /// Below a tenth of a point of bar there is nothing left to draw.
    private static let flat = 0.01

    init(engine: KoanEngine) {
        self.engine = engine
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

    /// The bands as of `now`, brought forward from the last frame that asked.
    ///
    /// Every indicator on screen calls this on every frame it draws and the
    /// first one does the work: the rest are the same frame, and a spectrum
    /// sampled twice in one frame would be two different answers to the same
    /// question.
    func bands(at now: TimeInterval, playing: Bool) -> [Double] {
        guard now > stamp else { return bands }
        // Clamped: a machine that slept owes the bars nothing.
        let elapsed = min(now - stamp, 0.25)
        stamp = now
        let fall = Self.remaining(halfLife: Self.release, over: elapsed)

        guard playing else {
            // Nothing is coming out of the speakers, so nothing is read: the
            // bars fall away on their own and the analyser, with no reader,
            // stands down while they do.
            for band in bands.indices { bands[band] *= fall }
            if bands.allSatisfy({ $0 < Self.flat }) {
                bands = [0, 0, 0]
                rest(true)
            }
            return bands
        }

        rest(false)
        let hold = Self.remaining(halfLife: Self.forget, over: elapsed)
        let frame = engine.vizLevels()
        let levels = [Double(frame.low), Double(frame.mid), Double(frame.high)]
        for band in levels.indices {
            ceiling[band] = max(levels[band], max(ceiling[band] * hold, Self.quietest))
            let level = min(levels[band] / ceiling[band], 1)
            bands[band] = max(level, bands[band] * fall)
        }
        return bands
    }

    /// Flip the one observed property from outside the body that noticed. It
    /// changes twice a pause rather than once a frame, and a view is midway
    /// through drawing when the change is spotted.
    private func rest(_ value: Bool) {
        guard settled != value else { return }
        Task { @MainActor in self.settled = value }
    }

    private func matchDisplay() {
        let main = NSApp.windows.first { $0.identifier?.rawValue == MainWindow.id }
        let screen = main?.screen ?? NSApp.keyWindow?.screen ?? NSScreen.main
        engine.setVizFps(fps: UInt8(clamping: screen?.maximumFramesPerSecond ?? 60))
    }

    /// What is left of a distance after `elapsed` at the given half-life.
    private static func remaining(halfLife: Double, over elapsed: Double) -> Double {
        pow(0.5, elapsed / halfLife)
    }
}
