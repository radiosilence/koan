import AppKit
import CoreImage
import SwiftUI

/// The wash, handed to the render server.
///
/// SwiftUI cannot express this without paying for it on the main thread.
/// `.offset` is a *layout* modifier, so animating one produces an
/// `AnimatableFrame` that the attribute graph has to evaluate every display
/// frame — and a `repeatForever` pair of those never stops. It cost about a
/// tenth of a core with nothing happening, and while the two-second dissolve
/// between records was running the whole app was unresponsive.
///
/// A `CABasicAnimation` on a layer's `transform` is committed once and then
/// belongs to Core Animation, which runs it in the render server — a different
/// process. The main thread does not see another frame of it, however long it
/// runs. This is the distinction React Native draws with `useNativeDriver`, and
/// it is the only way to have motion that costs nothing.
///
/// The blur is baked into the texture *once*, not left on the layer. A live
/// `CALayer.filters` looks free — it is the compositor's work, not ours — but
/// the compositor re-evaluates a layer's filters every frame it composites it,
/// and this layer is the size of the window and never stops moving.
///
/// Measured on an M1 Pro, this same layer in a 1400x900 window: a live filter
/// on a *settled* layer is indistinguishable from an empty window, and the
/// moment the drift starts it costs the render server about a seventh of a core
/// and ten points of GPU, continuously, for the wash on its own. Baked, the
/// drift reads as idle on both. So the filter is not free, and what it charges
/// for is the animation rather than its own presence.
///
/// This is not what made a tap slow — that survives at plain graphics, where no
/// wash is rendered at all (koan#380). Baking is simply the cheaper of the two.
struct DriftingWash: NSViewRepresentable {
    /// Nothing playing, or a record with no art, means no wash rather than a
    /// grey one.
    let image: NSImage?
    /// Whether the room is breathing. False settles it where it stands.
    let drifts: Bool

    func makeNSView(context: Context) -> WashView { WashView() }

    func updateNSView(_ view: WashView, context: Context) {
        view.show(image)
        view.drift(drifts)
    }
}

/// One layer holding the current cover, one holding the one before it, and a
/// crossfade between them. All three animations live in the render server.
final class WashView: NSView {
    /// The cover is blurred to mush, so it is rendered small and magnified
    /// afterwards — blurring a 360pt texture and scaling the result costs a
    /// fraction of blurring one the width of the window.
    nonisolated private static let side: CGFloat = 360

    /// How far the drift travels, and the overscan that lets it.
    ///
    /// What decides whether motion is visible is not its speed but how far it
    /// goes against how soft the thing moving is. Blurred at 14 points and
    /// magnified about five times, the wash has no feature narrower than eighty
    /// points on screen, so travel has to be read in multiples of that. These
    /// reach about four of them.
    ///
    /// `near` is a floor, not a taste: at full reach the offset carries the
    /// texture 12% of the window sideways and the rotation eats another 3.5%,
    /// and the scale has to keep the texture's own edge out of frame throughout.
    private static let near: CGFloat = 1.38
    private static let far: CGFloat = 1.58
    private static let reach: CGFloat = 0.12
    private static let rise: CGFloat = 0.10

    /// Three incommensurate periods, so the drift never arrives back where it
    /// started and never reads as a loop.
    private static let periods = (scale: 13.0, rotation: 19.0, position: 23.0)

    private let current = CALayer()
    private let previous = CALayer()
    private var shown: NSImage?
    private var drifting = false

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.masksToBounds = true
        for texture in [previous, current] {
            texture.contentsGravity = .resizeAspectFill
            texture.masksToBounds = false
            layer?.addSublayer(texture)
        }
        previous.opacity = 0
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not from a nib") }

    /// The layers fill the window, with room to move.
    ///
    /// Sized here rather than magnified by a transform, which is the whole
    /// point: the magnification used to *be* one of the drift's animations, so
    /// anything that skipped installing them — the graphics setting turned
    /// down and back up, motion reduced, nothing playing — left the layer at
    /// its natural size and drew a small blurred square in the middle of the
    /// window. A wash that fills its view by construction cannot be made to
    /// stop filling it by an animation that did not run.
    ///
    /// `near` is the overscan the drift travels inside: at full reach the
    /// offset carries the texture 12% of the window sideways and the rotation
    /// eats about another 3.5%, so the layer has to be wide enough that its own
    /// edge stays out of frame throughout.
    override func layout() {
        super.layout()
        let box = bounds.insetBy(
            dx: -bounds.width * (Self.near - 1) / 2,
            dy: -bounds.height * (Self.near - 1) / 2
        )
        // Frame changes must not be animated — an implicit animation here would
        // fight the drift and re-commit it on every resize.
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        current.frame = box
        previous.frame = box
        CATransaction.commit()
        if drifting { start() }
    }

    /// Swap in a new cover, dissolving from the old one over long enough that
    /// you notice the room has changed colour without catching it changing.
    ///
    /// The *new* layer fades in, on top of the old one holding station
    /// underneath. Fading the old one out instead does nothing visible: the new
    /// one is above it and already opaque, so the change lands as a cut.
    func show(_ image: NSImage?) {
        guard image !== shown else { return }
        shown = image
        generation &+= 1
        let mine = generation
        guard let image else { return install(nil) }
        Task { [weak self] in
            // Off the main thread and off the cooperative pool: this is a
            // Gaussian blur over a whole sleeve, once per record.
            let baked = await ImageWork.onCPU { Self.bake(image) }
            guard let self, self.generation == mine else { return }
            self.install(baked)
        }
    }

    /// Swap in a new cover, dissolving from the old one over long enough that
    /// you notice the room has changed colour without catching it changing.
    ///
    /// The *new* layer fades in, on top of the old one holding station
    /// underneath. Fading the old one out instead does nothing visible: the new
    /// one is above it and already opaque, so the change lands as a cut.
    private func install(_ baked: CGImage?) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        previous.contents = current.contents
        previous.opacity = current.contents == nil ? 0 : 1
        current.contents = baked
        CATransaction.commit()

        let dissolve = CABasicAnimation(keyPath: "opacity")
        dissolve.fromValue = 0
        dissolve.toValue = 1
        dissolve.duration = 2
        dissolve.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
        current.add(dissolve, forKey: "dissolve")
    }

    /// Blur and saturate, once, into a bitmap the compositor only has to move.
    ///
    /// The radius is in the texture's own pixels rather than the points it is
    /// drawn at, and the image is clamped first — an unclamped blur samples
    /// transparent black past the edge and leaves the sleeve with a soft dark
    /// border all the way round.
    ///
    /// It is measured against `side`, so the wash is blurred at 14 points of a
    /// 360pt reference and then magnified along with the texture. A live filter
    /// blurs *after* magnification, so 14 points there is a fifth of this blur
    /// and a different picture — matching it would take a radius around 70, and
    /// the live filter costs more at that radius, not less.
    private nonisolated static func bake(_ image: NSImage) -> CGImage? {
        var rect = NSRect(origin: .zero, size: image.size)
        guard let source = image.cgImage(forProposedRect: &rect, context: nil, hints: nil)
        else { return nil }
        let extent = CGRect(x: 0, y: 0, width: source.width, height: source.height)
        let radius = 14 * Double(source.width) / Double(side)
        let output = CIImage(cgImage: source)
            .applyingFilter("CIColorControls", parameters: [kCIInputSaturationKey: 1.6])
            .clampedToExtent()
            .applyingFilter("CIGaussianBlur", parameters: [kCIInputRadiusKey: radius])
            .cropped(to: extent)
        return ciContext.createCGImage(output, from: extent)
    }

    /// One context for the app. Building one per blur is where the expense of
    /// Core Image actually is.
    nonisolated private static let ciContext = CIContext(options: [.useSoftwareRenderer: false])

    /// Which cover is wanted, so a blur that finishes after the record moved on
    /// is dropped rather than drawn.
    private var generation = 0

    func drift(_ on: Bool) {
        guard on != drifting else { return }
        drifting = on
        if on { start() } else { settle() }
    }

    /// Committed once. Everything after this happens in the render server.
    ///
    /// Scales, turns and slides about the pose the layer already holds, so the
    /// wash is right whether these are running or not.
    private func start() {
        guard bounds.width > 0 else { return }
        for texture in [current, previous] {
            texture.removeAllAnimations()
            texture.add(
                Self.breathe(
                    "transform.scale",
                    from: 1,
                    to: Self.far / Self.near,
                    period: Self.periods.scale
                ),
                forKey: "scale"
            )
            texture.add(
                Self.breathe(
                    "transform.rotation.z",
                    from: -3 * Double.pi / 180,
                    to: 3 * Double.pi / 180,
                    period: Self.periods.rotation
                ),
                forKey: "rotation"
            )
            let centre = CGPoint(x: bounds.midX, y: bounds.midY)
            texture.add(
                Self.breathe(
                    "position",
                    from: NSValue(point: CGPoint(
                        x: centre.x - bounds.width * Self.reach,
                        y: centre.y - bounds.height * Self.rise
                    )),
                    to: NSValue(point: CGPoint(
                        x: centre.x + bounds.width * Self.reach,
                        y: centre.y + bounds.height * Self.rise
                    )),
                    period: Self.periods.position
                ),
                forKey: "position"
            )
        }
    }

    /// Playback stopping lets the room come to rest rather than stopping it
    /// mid-breath: the layer keeps whatever the animation had reached and eases
    /// back from there.
    private func settle() {
        for texture in [current, previous] {
            // Where the drift had actually reached, rather than where the model
            // says it is — otherwise removing the animation snaps the layer back
            // and the room stops dead instead of coming to rest.
            let held = texture.presentation()
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            if let held {
                texture.transform = held.transform
                texture.position = held.position
            }
            texture.removeAllAnimations()
            CATransaction.commit()

            CATransaction.begin()
            CATransaction.setAnimationDuration(2)
            CATransaction.setAnimationTimingFunction(
                CAMediaTimingFunction(name: .easeInEaseOut)
            )
            texture.transform = CATransform3DIdentity
            texture.position = CGPoint(x: bounds.midX, y: bounds.midY)
            CATransaction.commit()
        }
    }

    private static func breathe(
        _ keyPath: String,
        from: Any,
        to: Any,
        period: Double
    ) -> CABasicAnimation {
        let animation = CABasicAnimation(keyPath: keyPath)
        animation.fromValue = from
        animation.toValue = to
        animation.duration = period
        animation.autoreverses = true
        animation.repeatCount = .infinity
        animation.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
        // Survives the window being occluded or the app being hidden, which
        // otherwise removes the animation and leaves the wash parked.
        animation.isRemovedOnCompletion = false
        return animation
    }

}
