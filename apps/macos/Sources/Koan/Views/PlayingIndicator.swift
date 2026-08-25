import SwiftUI

/// The bars that mark whatever is playing. They dance while the transport
/// runs and settle when it stops, so the row says both "this one" and "still
/// going" without a second glyph.
struct PlayingIndicator: View {
    let isPlaying: Bool

    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private struct Bar {
        /// One rise and fall. Three incommensurate periods, so the bars never
        /// fall back into step and the group never reads as a loop.
        let period: Double
        /// Offset into the first cycle, so they don't all start together.
        let phase: Double
        /// Where a still bar sits, so a settled indicator still reads as bars.
        let rest: Double
    }

    private static let bars = [
        Bar(period: 0.61, phase: 0.00, rest: 0.75),
        Bar(period: 0.43, phase: 0.17, rest: 0.35),
        Bar(period: 0.53, phase: 0.31, rest: 0.60),
    ]

    private static let minHeight = 3.0
    private static let maxHeight = 11.0

    /// Both ends of the dance. Flipped once, then left alone — the animations
    /// below ping-pong off it, which is what keeps this off the main thread.
    @State private var lifted = false

    /// Scale rather than height, and one animation per bar rather than a
    /// `TimelineView` re-rendering at 30fps. Every tick of that invalidated
    /// layout, and a full window Auto Layout pass thirty times a second was a
    /// third of a core to draw nine points of bar. These hand the
    /// interpolation to CoreAnimation, which runs them off the main thread and
    /// keeps running them when the window is minimised, at no cost to us.
    var body: some View {
        HStack(alignment: .bottom, spacing: 2) {
            ForEach(Array(Self.bars.enumerated()), id: \.offset) { _, bar in
                Capsule(style: .continuous)
                    .frame(width: 2, height: Self.maxHeight)
                    .scaleEffect(y: scale(of: bar), anchor: .bottom)
                    .animation(motion(of: bar), value: lifted)
            }
        }
        .frame(height: Self.maxHeight, alignment: .bottom)
        .foregroundStyle(.tint)
        .accessibilityLabel(isPlaying ? "Playing" : "Paused")
        .onAppear { lifted = dances }
        .onChange(of: dances) { _, now in lifted = now }
    }

    private var dances: Bool { isPlaying && !reduceMotion }

    private func scale(of bar: Bar) -> Double {
        guard dances else { return fraction(bar.rest) }
        return lifted ? 1 : fraction(0)
    }

    private func motion(of bar: Bar) -> Animation {
        dances
            ? .easeInOut(duration: bar.period)
                .repeatForever(autoreverses: true)
                .delay(bar.phase)
            // Settling is a plain ease: playback stopping should let the bars
            // come to rest, not stop them mid-step.
            : .easeOut(duration: 0.3)
    }

    /// A 0–1 level as a fraction of the full bar height.
    private func fraction(_ level: Double) -> Double {
        (Self.minHeight + level * (Self.maxHeight - Self.minHeight)) / Self.maxHeight
    }
}
