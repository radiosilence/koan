import SwiftUI

/// The bars that mark whatever is playing. They dance while the transport
/// runs and freeze where they stand when it pauses, so the row says both
/// "this one" and "still going" without a second glyph.
///
/// The dance is a pair of sine waves and the music modulates them rather than
/// driving them: a chorus makes the bars swell, a quiet passage settles them
/// low and slow. Heights come from the carrier, which cannot spike, so the
/// indicator says *what* is playing without ever reading as jitter.
struct PlayingIndicator: View {
    let isPlaying: Bool

    @Environment(PlayingLevels.self) private var levels
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var watching = false

    private struct Bar {
        let speed: Double
        let phase: Double
        /// Where a still bar sits, so a frozen indicator still reads as bars —
        /// and, once the music is driving them, what the swing narrows toward.
        let rest: Double
    }

    /// One per band, low to high: three bars means three bands, and bars that
    /// rise and fall in lockstep read instantly as fake.
    private static let bars = [
        Bar(speed: 5.1, phase: 0.0, rest: 0.75),
        Bar(speed: 7.3, phase: 1.7, rest: 0.35),
        Bar(speed: 6.2, phase: 3.4, rest: 0.6),
    ]

    private static let minHeight = 3.0
    private static let maxHeight = 11.0
    private static let barWidth = 2.0
    private static let spacing = 2.0
    private static let width =
        Double(bars.count) * barWidth + Double(bars.count - 1) * spacing

    /// Whether there is anything to follow. Reduce Motion keeps the still bars,
    /// and still bars need no analyser.
    private var live: Bool { isPlaying && !reduceMotion }

    var body: some View {
        // At the rate the levels arrive, not the rate the display can manage.
        // Every tick is a SwiftUI graph update, and in this window each of
        // those costs a whole-window Auto Layout pass and a re-render of the
        // transport's glass — so a frame that redraws numbers that have not
        // changed is not free, it is the expensive half of the work.
        TimelineView(.animation(minimumInterval: PlayingLevels.interval, paused: !live)) { timeline in
            let now = timeline.date.timeIntervalSinceReferenceDate
            // Drawn rather than sized. Heights driven through `frame(height:)`
            // invalidated layout on every frame, and AppKit answered each one
            // with a full window Auto Layout pass — a third of a core to move
            // nine points of bar, whether or not the window was on screen. The
            // canvas is a fixed box; only its pixels change.
            Canvas { context, size in
                for (band, bar) in Self.bars.enumerated() {
                    let height = height(of: bar, band: band, at: now)
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

    private func height(of bar: Bar, band: Int, at now: Double) -> Double {
        guard !reduceMotion else {
            return Self.minHeight + bar.rest * (Self.maxHeight - Self.minHeight)
        }
        // Two frequencies with an irrational ratio: the bars never fall back
        // into a loop the eye can catch. The phase comes from the shared clock,
        // so every indicator on screen moves as one and the music can lean on
        // the rate without the argument jumping.
        let phase = levels.phase(at: now)
        let wave = sin(phase * bar.speed + bar.phase) * 0.6
            + sin(phase * bar.speed * 1.618 + bar.phase * 2) * 0.4
        let carrier = (wave + 1) / 2
        // The band pulls the swing in toward rest rather than setting a height.
        // Clamped because a bar is a drawn rectangle: the arithmetic keeps
        // itself inside 0...1 today, but a level that ever stepped outside it
        // would become a capsule with a negative height rather than a wrong one.
        let level = (bar.rest + (carrier - bar.rest) * levels.travel[band]).clamped()
        return Self.minHeight + level * (Self.maxHeight - Self.minHeight)
    }
}
