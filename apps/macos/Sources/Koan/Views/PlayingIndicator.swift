import SwiftUI

/// The bars that mark whatever is playing: a three-column spectrum analyser,
/// nine points tall. Heights are the low, mid and high bands as the analyser
/// reports them, so the row says both "this one" and what it sounds like — and
/// a silent passage is flat, because that is what is coming out of the
/// speakers. Pausing lets them fall rather than freezing them, the way the
/// TUI's spectrum settles.
struct PlayingIndicator: View {
    let isPlaying: Bool

    @Environment(PlayingLevels.self) private var levels
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    /// The queue stays mounted while you are elsewhere, so an indicator can be
    /// off screen without ever disappearing. Off stage it stops drawing, which
    /// is also what stops it reading the analyser.
    @Environment(\.onStage) private var onStage
    @AppStorage("graphics") private var graphics = Graphics.full

    /// The shape a still indicator holds — staggered, so it reads as bars
    /// rather than as a broken one. Nothing moving needs an analyser.
    private static let resting = [0.75, 0.35, 0.6]

    private static let minHeight = 3.0
    private static let maxHeight = 11.0
    private static let barWidth = 2.0
    private static let spacing = 2.0
    private static let width =
        Double(resting.count) * barWidth + Double(resting.count - 1) * spacing

    /// Whether the bars hold their shape instead of following the music.
    /// Reduce Motion asks for it, and so does the bottom of the graphics
    /// ladder.
    private var still: Bool { reduceMotion || !graphics.animatesIndicators }

    /// Whether there is anything left to draw. A pause keeps the timeline
    /// running until the bars have fallen, and nothing runs after that: no
    /// frame, no read, no analyser.
    private var live: Bool { onStage && !still && (isPlaying || !levels.settled) }

    var body: some View {
        // The display's own cadence. Each tick is a SwiftUI graph update and
        // the analyser is told to run at the same rate, so a tick is one new
        // set of numbers rather than a redraw of numbers that have not changed.
        TimelineView(.animation(paused: !live)) { timeline in
            let bands = levels.bands(
                at: timeline.date.timeIntervalSinceReferenceDate, playing: isPlaying)
            // Drawn rather than sized. Heights driven through `frame(height:)`
            // invalidated layout on every frame, and AppKit answered each one
            // with a full window Auto Layout pass — a third of a core to move
            // nine points of bar, whether or not the window was on screen. The
            // canvas is a fixed box; only its pixels change.
            Canvas { context, size in
                for band in Self.resting.indices {
                    // Clamped because a bar is a drawn rectangle: the level
                    // keeps itself inside 0...1 today, but one that ever
                    // stepped outside it would become a capsule with a
                    // negative height rather than a wrong one.
                    let level = still ? Self.resting[band] : bands[band].clamped()
                    let height = Self.minHeight + level * (Self.maxHeight - Self.minHeight)
                    let rect = CGRect(
                        x: Double(band) * (Self.barWidth + Self.spacing),
                        y: size.height - height,
                        width: Self.barWidth,
                        height: height
                    )
                    context.fill(Capsule(style: .continuous).path(in: rect), with: .foreground)
                }
            }
        }
        .frame(width: Self.width, height: Self.maxHeight)
        .foregroundStyle(.tint)
        .accessibilityLabel(isPlaying ? "Playing" : "Paused")
    }
}
