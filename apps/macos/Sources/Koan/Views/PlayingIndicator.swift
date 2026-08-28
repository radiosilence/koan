import SwiftUI

/// The bars that mark whatever is playing: a three-column spectrum analyser,
/// nine points tall. Heights are the low, mid and high bands as the analyser
/// reports them, so the row says both "this one" and what it sounds like — and
/// a silent passage is flat, because that is what is coming out of the
/// speakers. Pausing lets them fall rather than freezing them mid-swing.
///
/// There is no timeline and no clock. A published frame moves `bands`, which
/// invalidates this and nothing else; no frame means no redraw. An indicator
/// that is off stage or holding still never reads them, which is what
/// unsubscribes it.
struct PlayingIndicator: View {
    let isPlaying: Bool

    @Environment(PlayingLevels.self) private var levels
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    /// The queue stays mounted while you are elsewhere, so an indicator can be
    /// off screen without ever disappearing.
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

    var body: some View {
        // Read only where it is drawn: not reading `bands` is what keeps an
        // off-stage or still indicator out of the redraws entirely.
        let bands = still || !onStage ? Self.resting : levels.bands
        // Drawn rather than sized. Heights driven through `frame(height:)`
        // invalidated layout on every frame, and AppKit answered each one with
        // a full window Auto Layout pass — a third of a core to move nine
        // points of bar, whether or not the window was on screen. The canvas
        // is a fixed box; only its pixels change.
        Canvas { context, size in
            for band in Self.resting.indices {
                // Clamped because a bar is a drawn rectangle: the level keeps
                // itself inside 0...1 today, but one that ever stepped outside
                // it would become a capsule with a negative height rather than
                // a wrong one.
                let height =
                    Self.minHeight + bands[band].clamped() * (Self.maxHeight - Self.minHeight)
                let rect = CGRect(
                    x: Double(band) * (Self.barWidth + Self.spacing),
                    y: size.height - height,
                    width: Self.barWidth,
                    height: height
                )
                context.fill(Capsule(style: .continuous).path(in: rect), with: .foreground)
            }
        }
        .frame(width: Self.width, height: Self.maxHeight)
        .foregroundStyle(.tint)
        .accessibilityLabel(isPlaying ? "Playing" : "Paused")
    }
}
