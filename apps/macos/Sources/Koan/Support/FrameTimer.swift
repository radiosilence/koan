import AppKit
import OSLog
import QuartzCore

/// What happens between a page's body and the pixels.
///
/// A signpost inside a `body` says when SwiftUI *evaluated* a page. Opening a
/// record evaluates in single-figure milliseconds and takes a third of a second
/// to appear, and everything in between — layout, the CoreAnimation commit, the
/// render server — is after the last line any view gets to run. Nothing
/// declared in a view can observe it, which is why four rounds of hypothesis
/// about that gap were argued from measurements that stopped before it.
///
/// A display link can observe it. It ticks on the main run loop at the top of
/// each frame, so the first tick after the body ran belongs to the first frame
/// that could have carried it, and a main thread stuck in a commit delays that
/// tick by exactly as long as it was stuck. The link is started at the tap
/// rather than at the body: a link only just added to the run loop takes a
/// moment to find the display, and starting it later would bill that moment to
/// the app.
///
/// Three numbers come out. How long koan took to work out what to draw, how
/// long until the frame that could show it, and every stall after that until
/// the run loop is running at cadence again — because a record does not arrive
/// in one commit, and the ones after the first are what you are still waiting
/// through.
///
/// Read it without Instruments:
///
///     log stream --level info --predicate 'subsystem == "cc.blit.koan"'
///
/// or as a `tap-to-frame` region in Points of Interest, nested inside the
/// `click-to-album` region the navigator emits.
@MainActor
final class FrameTimer: NSObject {
    static let shared = FrameTimer()

    /// A gap longer than this is a stall rather than a frame. Two frames at
    /// 60Hz — anything under it is the run loop keeping up, whatever the
    /// display's refresh rate.
    private static let stall = Duration.milliseconds(34)
    /// Enough ticks at cadence to call it settled.
    private static let calm = 4

    private let log = Logger(subsystem: "cc.blit.koan", category: "frames")
    private var link: CADisplayLink?
    private var move: Move?

    private struct Move {
        let started: ContinuousClock.Instant
        let signpost: OSSignpostIntervalState
        /// How long until the page's body ran. Nil until it has.
        var evaluated: Duration?
        /// Every tick since the tap, as an offset from it.
        var ticks: [Duration] = []
        /// TEMPORARY (#380): what landed when, to name the second commit.
        var notes: [(String, Duration)] = []
    }

    /// A gesture that should end in a new page. Supersedes any move still in
    /// flight — tapping again is the user having given up on the last one.
    func begin() {
        discard()
        move = Move(
            started: .now,
            signpost: Trace.signposter.beginInterval(
                "tap-to-frame",
                id: Trace.signposter.makeSignpostID()
            )
        )
        guard let window = NSApp.keyWindow ?? NSApp.mainWindow else { return }
        let link = window.displayLink(target: self, selector: #selector(tick))
        link.add(to: .main, forMode: .common)
        self.link = link
    }

    /// The page has been evaluated. Everything after this is the part no view
    /// can see.
    func evaluated() {
        guard var move, move.evaluated == nil else { return }
        move.evaluated = .now - move.started
        self.move = move
    }

    /// TEMPORARY (#380): mark a state change against the tap that caused it.
    /// `at` for something that happened off the main actor, so the mark is when
    /// it happened rather than when the main actor got round to saying so.
    func note(_ name: String, at instant: ContinuousClock.Instant = .now) {
        guard var move else { return }
        move.notes.append((name, instant - move.started))
        self.move = move
    }

    @objc private func tick() {
        guard var move else { return discard() }
        move.ticks.append(.now - move.started)
        self.move = move
        guard let evaluated = move.evaluated, settled(move.ticks) else { return }
        Trace.signposter.endInterval("tap-to-frame", move.signpost)
        report(evaluated: evaluated, ticks: move.ticks, notes: move.notes)
        discard()
    }

    /// The run loop is back at cadence: the last few ticks all came within a
    /// frame or two of each other, so whatever the page cost, it has been paid.
    private func settled(_ ticks: [Duration]) -> Bool {
        guard ticks.count > Self.calm else { return false }
        return zip(ticks.dropFirst(ticks.count - Self.calm), ticks.suffix(Self.calm - 1))
            .allSatisfy { $0 - $1 < Self.stall }
    }

    private func report(evaluated: Duration, ticks: [Duration], notes: [(String, Duration)]) {
        // The frame that could have carried the page, and every stall the run
        // loop took after it — a record arrives in more than one commit, and
        // the later ones are still time spent looking at the old page.
        let frame = ticks.first { $0 > evaluated } ?? evaluated
        let stalls = zip(ticks.dropFirst(), ticks)
            .filter { $0 > frame && $0 - $1 >= Self.stall }
            .map { Self.ms($0 - $1) }
        log.info(
            """
            tap-to-frame \(Self.ms(frame), privacy: .public)ms \
            (body \(Self.ms(evaluated), privacy: .public)ms, \
            draw \(Self.ms(frame - evaluated), privacy: .public)ms) \
            then \(stalls.isEmpty ? "no stalls" : stalls.joined(separator: "ms, ") + "ms",
                   privacy: .public)
            """
        )
        let landed = notes.map { "\($0.0)@\(Self.ms($0.1))" }.joined(separator: " ")
        let series = ticks.map { Self.ms($0) }.joined(separator: " ")
        log.info("  landed \(landed, privacy: .public)")
        log.info("  ticks \(series, privacy: .public)")
    }

    /// The link only runs between a tap and the frame that answers it; leaving
    /// it ticking would wake the main thread sixty times a second for nothing.
    private func discard() {
        link?.invalidate()
        link = nil
        move = nil
    }

    private static func ms(_ duration: Duration) -> String {
        let millis = Double(duration.components.seconds) * 1000
            + Double(duration.components.attoseconds) / 1e15
        return String(format: "%.1f", millis)
    }
}
