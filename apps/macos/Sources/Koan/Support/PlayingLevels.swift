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

    /// How far each bar should swing from where it rests, 0...1, low band to
    /// high. The music sets this and nothing else — it never reaches a height
    /// directly, so there is no path from a transient to a jump.
    @ObservationIgnored private(set) var travel = idle

    /// The carrier's phase at `stamp`, and how fast it is advancing. Read it
    /// through `phase(at:)` rather than directly: a frame and a poll are on
    /// separate clocks even at the same nominal rate, and winding the phase on
    /// to the moment being drawn keeps the motion continuous rather than
    /// stepped at whatever rate the frames actually arrive.
    @ObservationIgnored private var phase = 0.0
    @ObservationIgnored private var stamp = Date().timeIntervalSinceReferenceDate
    @ObservationIgnored private var rate = 1.0

    private var watchers = 0
    private var poll: Task<Void, Never>?

    /// The loudest each band has been lately. Each band is judged against its
    /// own recent range rather than against full scale, which is what stops a
    /// track mastered quiet getting a limper indicator than a loud one — and
    /// incidentally undoes the analyser's A-weighting tilt, which otherwise
    /// leaves the bass bar permanently the sluggish one.
    private var ceiling = [quietest, quietest, quietest]

    /// Set once the analyser has shown us any audio at all. Until then the bars
    /// run the plain carrier: a track still buffering is not a quiet one.
    private var heard = false

    /// Full travel — the motion the bars had before any of this. Where they go
    /// when there is nothing to go on.
    private static let idle = [1.0, 1.0, 1.0]
    /// A band below this is room tone, and never sets a ceiling.
    private static let quietest = 0.12
    /// Fast up so a transient lands, slow down so nothing snaps to zero between
    /// beats. Sluggish and legible beats accurate and spiky at eleven points.
    private static let attack = 0.05
    private static let release = 0.35
    /// How long a band takes to forget a loud passage.
    private static let forget = 4.0
    /// The least the bars ever move. A state marker first: whatever the music
    /// is doing, the row that is playing has to still say so at a glance.
    private static let floor = 0.3
    /// How much of the bars' rate the music gets to move.
    private static let rateSwing = 0.4
    /// How often the levels are resampled — and, since new numbers are the
    /// only reason to redraw, the rate the indicators run at too.
    static let interval = 1.0 / 30.0

    init(engine: KoanEngine) {
        self.engine = engine
    }

    /// The carrier's phase now, wound on from the last sample at the rate the
    /// music last set.
    func phase(at now: TimeInterval) -> Double {
        phase + (now - stamp) * rate
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
        // Travel is left where it stands: the bars freeze mid-swing when the
        // transport stops, and a redraw while paused should not move them.
        heard = false
    }

    private func sample() {
        let now = Date().timeIntervalSinceReferenceDate
        // Clamped: a machine that slept owes the bars nothing.
        let elapsed = min(max(now - stamp, 0), 0.25)
        let wound = phase(at: now)
        let frame = engine.vizLevels()
        let bands = [Double(frame.low), Double(frame.mid), Double(frame.high)]
        heard = heard || bands.contains { $0 > Self.quietest }

        defer {
            phase = wound
            stamp = now
        }

        guard heard else {
            travel = Self.idle
            rate = 1
            return
        }

        let hold = Self.remaining(halfLife: Self.forget, over: elapsed)
        var next = travel
        for band in bands.indices {
            ceiling[band] = max(bands[band], max(ceiling[band] * hold, Self.quietest))
            let energy = min(bands[band] / ceiling[band], 1)
            let target = Self.floor + (1 - Self.floor) * energy
            let halfLife = target > travel[band] ? Self.attack : Self.release
            next[band] =
                target + (travel[band] - target) * Self.remaining(halfLife: halfLife, over: elapsed)
        }
        travel = next

        let mean = next.reduce(0, +) / Double(next.count)
        rate = 1 + Self.rateSwing * (mean - Self.floor) / (1 - Self.floor)
    }

    /// What is left of a distance after `elapsed` at the given half-life.
    private static func remaining(halfLife: Double, over elapsed: Double) -> Double {
        pow(0.5, elapsed / halfLife)
    }
}
