import AppKit
import SwiftUI

/// The played extent and the head of the seek bar, as layers the render server
/// animates on its own.
///
/// SwiftUI could do this in two lines, and for the length of a track that is a
/// main-thread callback every frame to move a bar by a pixel — the app awake
/// for the whole of a fifty-five minute mix. A `CABasicAnimation` is handed
/// over once and runs in the render server, so between one anchor and the next
/// this process is not woken at all. That is the whole claim the anchor makes;
/// animating it here would have quietly cost what publishing the position used
/// to.
///
/// Nothing here is a clock either. `remaining` is how much of the track is
/// left, so the animation ends as the track does.
struct SeekProgress: NSViewRepresentable {
    /// Where the head is now, 0...1.
    let fraction: Double
    /// Seconds until the animation should reach the end. Zero for a bar that
    /// is not advancing — paused, stopped, or being dragged.
    let remaining: TimeInterval
    let thickness: CGFloat

    func makeNSView(context: Context) -> ProgressView {
        ProgressView(thickness: thickness)
    }

    func updateNSView(_ view: ProgressView, context: Context) {
        view.apply(fraction: fraction, remaining: remaining)
    }

    /// Two layers: the capsule of what has played, and the head that marks
    /// where that is on a track too long for the capsule to show it.
    final class ProgressView: NSView {
        private let played = CALayer()
        private let head = CALayer()
        private let thickness: CGFloat
        private var fraction = 0.0
        private var remaining = 0.0

        init(thickness: CGFloat) {
            self.thickness = thickness
            super.init(frame: .zero)
            wantsLayer = true
            // Grown from its leading edge, so widening it is one animatable
            // number rather than a width and a position that must agree.
            played.anchorPoint = CGPoint(x: 0, y: 0.5)
            played.cornerRadius = thickness / 2
            head.cornerRadius = thickness
            for sublayer in [played, head] {
                sublayer.actions = ["bounds": NSNull(), "position": NSNull()]
                layer?.addSublayer(sublayer)
            }
            paint()
        }

        @available(*, unavailable)
        required init?(coder: NSCoder) { fatalError("not from a nib") }

        override func layout() {
            super.layout()
            place()
        }

        override func viewDidChangeEffectiveAppearance() {
            super.viewDidChangeEffectiveAppearance()
            paint()
        }

        func apply(fraction: Double, remaining: TimeInterval) {
            self.fraction = fraction.clamped()
            self.remaining = remaining
            place()
        }

        private func paint() {
            let colour = NSColor.labelColor.cgColor
            played.backgroundColor = colour
            head.backgroundColor = colour
        }

        /// Put both layers where the fraction says, then — if the track is
        /// still running — hand them the rest of it.
        private func place() {
            played.removeAllAnimations()
            head.removeAllAnimations()

            let width = bounds.width
            let diameter = thickness * 2
            let travel = max(0, width - diameter)
            let middle = bounds.midY

            CATransaction.begin()
            CATransaction.setDisableActions(true)
            played.bounds = CGRect(x: 0, y: 0, width: width * fraction, height: thickness)
            played.position = CGPoint(x: 0, y: middle)
            head.bounds = CGRect(x: 0, y: 0, width: diameter, height: diameter)
            head.position = CGPoint(x: travel * fraction + diameter / 2, y: middle)
            CATransaction.commit()

            guard remaining > 0, width > 0 else { return }
            played.add(
                grow(to: CGRect(x: 0, y: 0, width: width, height: thickness), from: played.bounds),
                forKey: "seek")
            head.add(
                slide(to: CGPoint(x: travel + diameter / 2, y: middle), from: head.position),
                forKey: "seek")
        }

        private func grow(to end: CGRect, from start: CGRect) -> CABasicAnimation {
            let animation = CABasicAnimation(keyPath: "bounds")
            animation.fromValue = start
            animation.toValue = end
            return settled(animation)
        }

        private func slide(to end: CGPoint, from start: CGPoint) -> CABasicAnimation {
            let animation = CABasicAnimation(keyPath: "position")
            animation.fromValue = start
            animation.toValue = end
            return settled(animation)
        }

        /// Linear, and left in place when it finishes: a track that runs to its
        /// end leaves the bar full until the next one says otherwise.
        private func settled(_ animation: CABasicAnimation) -> CABasicAnimation {
            animation.duration = remaining
            animation.timingFunction = CAMediaTimingFunction(name: .linear)
            animation.fillMode = .forwards
            animation.isRemovedOnCompletion = false
            return animation
        }
    }
}
