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

    /// Whether there is anything to follow. Reduce Motion keeps the still bars,
    /// and still bars need no analyser.
    private var live: Bool { isPlaying && !reduceMotion }

    var body: some View {
        TimelineView(.animation(minimumInterval: 1 / 60, paused: !live)) { timeline in
            let now = timeline.date.timeIntervalSinceReferenceDate
            HStack(alignment: .bottom, spacing: 2) {
                ForEach(Array(Self.bars.enumerated()), id: \.offset) { band, bar in
                    Capsule(style: .continuous)
                        .frame(width: 2, height: height(of: bar, band: band, at: now))
                }
            }
        }
        .frame(height: Self.maxHeight, alignment: .bottom)
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
        let level = bar.rest + (carrier - bar.rest) * levels.travel[band]
        return Self.minHeight + level * (Self.maxHeight - Self.minHeight)
    }
}
