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
    private let mirror: EngineMirror

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

    /// Where the play head was last look, and when it last moved.
    ///
    /// Silence and a stall reach the analyser as the same thing — zeros — and
    /// they mean opposite things: a track still buffering has nothing to say
    /// yet, while a track that opens on ten seconds of silence is saying it.
    /// The play head advances only as audio leaves the engine, so it is what
    /// tells the two apart. Until it moves the bars run the plain carrier;
    /// once it does, the analyser is believed, silence included.
    private var seen: UInt64 = 0
    private var movedAt = 0.0

    /// Full travel — the motion the bars had before any of this. Where they go
    /// when there is nothing to go on.
    private static let idle = [1.0, 1.0, 1.0]
    /// A band below this is room tone, and never sets a ceiling.
    private static let quietest = 0.12
    /// How long a band takes to fall away. Rises are not damped at all: this
    /// is the law the TUI's spectrum runs on — up on the frame it happens,
    /// down on a half-life so nothing snaps to zero between beats. The
    /// analyser has already smoothed the bars at 50ms, and a second slow
    /// filter here only made the indicator late to its own music.
    private static let release = 0.15
    /// How long a band takes to forget a loud passage.
    private static let forget = 4.0
    /// How long the play head may sit still before it counts as stalled. The
    /// engine publishes it ten times a second, so a couple of missed updates
    /// are ordinary and only a real stall lasts past this.
    private static let stall = 0.5
    /// How much of the bars' rate the music gets to move.
    private static let rateSwing = 0.4
    /// How often the levels are resampled — and, since new numbers are the
    /// only reason to redraw, the rate the indicators run at too.
    static let interval = 1.0 / 30.0

    init(engine: KoanEngine, mirror: EngineMirror) {
        self.engine = engine
        self.mirror = mirror
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
        // Where the play head stands, not that it has moved: a track that is
        // still buffering when the first indicator appears has told us nothing.
        seen = mirror.positionMs
        movedAt = 0
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
    }

    private func sample() {
        let now = Date().timeIntervalSinceReferenceDate
        // Clamped: a machine that slept owes the bars nothing.
        let elapsed = min(max(now - stamp, 0), 0.25)
        let wound = phase(at: now)
        let position = mirror.positionMs
        if position != seen {
            seen = position
            movedAt = now
        }
        let frame = engine.vizLevels()
        let bands = [Double(frame.low), Double(frame.mid), Double(frame.high)]

        defer {
            phase = wound
            stamp = now
        }

        guard now - movedAt < Self.stall else {
            travel = Self.idle
            rate = 1
            return
        }

        let hold = Self.remaining(halfLife: Self.forget, over: elapsed)
        var next = travel
        for band in bands.indices {
            ceiling[band] = max(bands[band], max(ceiling[band] * hold, Self.quietest))
            // Silence is a level like any other: the bars settle onto their
            // resting heights and stay there until there is something to move
            // them. Still bars on a playing row are the truth, and the row is
            // still legible as the one playing — three bars at three heights.
            let target = min(bands[band] / ceiling[band], 1)
            let fallen = travel[band] * Self.remaining(halfLife: Self.release, over: elapsed)
            next[band] = max(target, fallen)
        }
        travel = next

        rate = 1 + Self.rateSwing * next.reduce(0, +) / Double(next.count)
    }

    /// What is left of a distance after `elapsed` at the given half-life.
    private static func remaining(halfLife: Double, over elapsed: Double) -> Double {
        pow(0.5, elapsed / halfLife)
    }
}
