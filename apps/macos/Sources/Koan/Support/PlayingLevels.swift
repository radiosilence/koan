import Foundation
import KoanFFI

/// The audio behind the playing indicators.
///
/// One source for the whole app: the queue and a track list can both have a
/// current row on screen, and they are watching the same music. Views take a
/// subscription while they need one, so with nothing on screen — or nothing
/// playing — nothing runs.
///
/// Nothing here is observed. These values move thirty times a second and an
/// observer would re-render at that rate; the indicators redraw off their own
/// display-linked timeline and read this when they do.
@MainActor
@Observable
final class PlayingLevels {
    private let engine: KoanEngine

    /// How high each bar stands, 0...1, low band to high. The spectrum, in
    /// three columns: what the analyser says is coming out of the speakers and
    /// nothing else. Silence is zero and reads flat.
    @ObservationIgnored private(set) var bands = [0.0, 0.0, 0.0]

    private var watchers = 0
    private var poll: Task<Void, Never>?
    private var stamp = Date().timeIntervalSinceReferenceDate

    /// The loudest each band has been lately. Each band is judged against its
    /// own recent range rather than against full scale, which is what stops a
    /// track mastered quiet getting a limper indicator than a loud one — and
    /// incidentally undoes the analyser's A-weighting tilt, which otherwise
    /// leaves the bass bar permanently the sluggish one.
    private var ceiling = [quietest, quietest, quietest]

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
    /// How often the levels are resampled — and, since new numbers are the
    /// only reason to redraw, the rate the indicators run at too.
    static let interval = 1.0 / 30.0

    init(engine: KoanEngine) {
        self.engine = engine
    }

    func watch() {
        watchers += 1
        if watchers == 1 { start() }
    }

    func unwatch() {
        watchers -= 1
        if watchers == 0 { stop() }
    }

    private func start() {
        stamp = Date().timeIntervalSinceReferenceDate
        poll = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(Self.interval))
                guard let self else { return }
                sample()
            }
        }
    }

    private func stop() {
        poll?.cancel()
        poll = nil
        // The bars are left where they stand: they freeze at their last
        // heights when the transport pauses, and a redraw while paused should
        // not move them.
    }

    private func sample() {
        let now = Date().timeIntervalSinceReferenceDate
        // Clamped: a machine that slept owes the bars nothing.
        let elapsed = min(max(now - stamp, 0), 0.25)
        stamp = now

        let frame = engine.vizLevels()
        let levels = [Double(frame.low), Double(frame.mid), Double(frame.high)]
        let hold = Self.remaining(halfLife: Self.forget, over: elapsed)
        let fall = Self.remaining(halfLife: Self.release, over: elapsed)

        for band in levels.indices {
            ceiling[band] = max(levels[band], max(ceiling[band] * hold, Self.quietest))
            let level = min(levels[band] / ceiling[band], 1)
            bands[band] = max(level, bands[band] * fall)
        }
    }

    /// What is left of a distance after `elapsed` at the given half-life.
    private static func remaining(halfLife: Double, over elapsed: Double) -> Double {
        pow(0.5, elapsed / halfLife)
    }
}
