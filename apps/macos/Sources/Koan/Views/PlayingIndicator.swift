import SwiftUI

/// The bars that mark whatever is playing: a three-column spectrum analyser,
/// nine points tall. Heights are the low, mid and high bands as the analyser
/// reports them, so the row says both "this one" and what it sounds like — and
/// a silent passage is flat, because that is what is coming out of the
/// speakers. They hold their last heights when the transport pauses.
struct PlayingIndicator: View {
    let isPlaying: Bool

    @Environment(PlayingLevels.self) private var levels
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    /// The queue stays mounted while you are elsewhere, so an indicator can be
    /// off screen without ever disappearing. Off stage it stops asking the
    /// analyser for levels and its timeline stops ticking.
    @Environment(\.onStage) private var onStage
    @AppStorage("graphics") private var graphics = Graphics.full
    @State private var watching = false

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

    /// Whether there is anything to follow. Still bars need no analyser, so
    /// this is also what decides whether anything polls it.
    private var live: Bool { isPlaying && !still && onStage }

    var body: some View {
        // At the rate the levels arrive, not the rate the display can manage.
        // Every tick is a SwiftUI graph update, and in this window each of
        // those costs a whole-window Auto Layout pass and a re-render of the
        // transport's glass — so a frame that redraws numbers that have not
        // changed is not free, it is the expensive half of the work.
        TimelineView(.animation(minimumInterval: PlayingLevels.interval, paused: !live)) { _ in
            // Drawn rather than sized. Heights driven through `frame(height:)`
            // invalidated layout on every frame, and AppKit answered each one
            // with a full window Auto Layout pass — a third of a core to move
            // nine points of bar, whether or not the window was on screen. The
            // canvas is a fixed box; only its pixels change.
            Canvas { context, size in
                for band in Self.resting.indices {
                    let height = height(of: band)
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
        .onAppear { watch(live) }
        .onDisappear { watch(false) }
        .onChange(of: live) { _, wanted in watch(wanted) }
    }

    private func watch(_ wanted: Bool) {
        guard wanted != watching else { return }
        watching = wanted
        if wanted {
            levels.watch()
        } else {
            levels.unwatch()
        }
    }

    private func height(of band: Int) -> Double {
        // Clamped because a bar is a drawn rectangle: the level keeps itself
        // inside 0...1 today, but one that ever stepped outside it would
        // become a capsule with a negative height rather than a wrong one.
        let level = still ? Self.resting[band] : levels.bands[band].clamped()
        return Self.minHeight + level * (Self.maxHeight - Self.minHeight)
    }
}
