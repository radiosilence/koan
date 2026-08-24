import SwiftUI

/// The bars that mark whatever is playing. They dance while the transport
/// runs and freeze where they stand when it pauses, so the row says both
/// "this one" and "still going" without a second glyph.
struct PlayingIndicator: View {
    let isPlaying: Bool

    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private struct Bar {
        let speed: Double
        let phase: Double
        /// Where a still bar sits, so a frozen indicator still reads as bars.
        let rest: Double
    }

    private static let bars = [
        Bar(speed: 5.1, phase: 0.0, rest: 0.75),
        Bar(speed: 7.3, phase: 1.7, rest: 0.35),
        Bar(speed: 6.2, phase: 3.4, rest: 0.6),
    ]

    private static let minHeight = 3.0
    private static let maxHeight = 11.0

    var body: some View {
        TimelineView(.animation(minimumInterval: 1 / 30, paused: !isPlaying || reduceMotion)) { timeline in
            let now = timeline.date.timeIntervalSinceReferenceDate
            HStack(alignment: .bottom, spacing: 2) {
                ForEach(Array(Self.bars.enumerated()), id: \.offset) { _, bar in
                    Capsule(style: .continuous)
                        .frame(width: 2, height: height(of: bar, at: now))
                }
            }
        }
        .frame(height: Self.maxHeight, alignment: .bottom)
        .foregroundStyle(.tint)
        .accessibilityLabel(isPlaying ? "Playing" : "Paused")
    }

    private func height(of bar: Bar, at now: Double) -> Double {
        guard !reduceMotion else {
            return Self.minHeight + bar.rest * (Self.maxHeight - Self.minHeight)
        }
        // Two frequencies with an irrational ratio: the bars never fall back
        // into a loop the eye can catch.
        let wave = sin(now * bar.speed + bar.phase) * 0.6
            + sin(now * bar.speed * 1.618 + bar.phase * 2) * 0.4
        let level = (wave + 1) / 2
        return Self.minHeight + level * (Self.maxHeight - Self.minHeight)
    }
}
