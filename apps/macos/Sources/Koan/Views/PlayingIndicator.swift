import SwiftUI

/// The bars that mark whatever is playing: a three-column spectrum analyser,
/// nine points tall. Heights are the low, mid and high bands as the analyser
/// reports them, so the row says both "this one" and what it sounds like — and
/// a silent passage is flat, because that is what is coming out of the
/// speakers. Pausing lets them fall rather than freezing them mid-swing.
///
/// There is no timeline and no clock, and there is no SwiftUI in the loop
/// either. The bars are layers, and `PlayingLevels` hands each frame straight
/// to them; this view's body runs when the tint or the stage changes and at
/// no other time. An indicator that is off stage or holding still detaches
/// itself, which is what lets the analyser park.
struct PlayingIndicator: View {
    let isPlaying: Bool

    @Environment(PlayingLevels.self) private var levels
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    /// The queue stays mounted while you are elsewhere, so an indicator can be
    /// off screen without ever disappearing.
    @Environment(\.onStage) private var onStage
    /// The bars draw in the room's colour, which `.tint` cannot hand an
    /// AppKit view — see `EnvironmentValues.roomTint`.
    @Environment(\.roomTint) private var tint
    @Environment(\.powerSaving) private var powerSaving

    /// Whether the bars follow the music. Reduce Motion and Low Power Mode ask
    /// them not to; off stage nobody is looking. Every graphics level lets
    /// them move: they cost next to nothing.
    private var live: Bool {
        onStage && !reduceMotion && !powerSaving
    }

    var body: some View {
        PlayingBars(live: live, tint: PlatformColor(tint), levels: levels)
            .frame(width: PlayingBarsView.width, height: PlayingBarsView.maxHeight)
            .accessibilityLabel(isPlaying ? "Playing" : "Paused")
    }
}

private struct PlayingBars: PlatformViewRepresentable {
    let live: Bool
    let tint: PlatformColor
    let levels: PlayingLevels

    typealias PlatformViewType = PlayingBarsView

    func makeView(context: Context) -> PlayingBarsView { PlayingBarsView() }

    func updateView(_ view: PlayingBarsView, context: Context) {
        view.tint = tint
        view.source = levels
        if live {
            levels.attach(view)
        } else {
            levels.detach(view)
            view.rest()
        }
    }

    static func dismantleView(_ view: PlayingBarsView, coordinator: ()) {
        view.source?.detach(view)
    }
}

/// Three capsules, moved by their bounds. Nothing else about them ever
/// changes, and a bounds change with actions disabled is the cheapest thing a
/// layer can be asked to do.
final class PlayingBarsView: LayerView {
    /// The shape a still indicator holds — staggered, so it reads as bars
    /// rather than as a broken one. Nothing moving needs an analyser.
    private static let resting = [0.75, 0.35, 0.6]

    private static let minHeight = 3.0
    static let maxHeight = 11.0
    private static let barWidth = 2.0
    private static let spacing = 2.0
    static let width =
        Double(resting.count) * barWidth + Double(resting.count - 1) * spacing

    private let bars = resting.map { _ in CALayer() }
    /// Who is feeding this, so being torn down can say so.
    weak var source: PlayingLevels?

    var tint: PlatformColor = .label {
        didSet {
            guard tint != oldValue else { return }
            paint()
        }
    }

    override init(frame: CGRect) {
        super.init(frame: frame)
        // Bars stand on the floor. AppKit's origin is already there; UIKit's is
        // at the top, and would hang them from the ceiling.
        #if !canImport(AppKit)
        hostLayer.isGeometryFlipped = true
        #endif
        for (index, bar) in bars.enumerated() {
            bar.cornerRadius = KoanTheme.radius(Self.barWidth / 2)
            // Grown from the foot, so a height is one number rather than a
            // height and a position that must agree.
            bar.anchorPoint = CGPoint(x: 0.5, y: 0)
            bar.position = CGPoint(
                x: Double(index) * (Self.barWidth + Self.spacing) + Self.barWidth / 2,
                y: 0
            )
            bar.actions = ["bounds": NSNull(), "position": NSNull(), "backgroundColor": NSNull()]
            hostLayer.addSublayer(bar)
        }
        paint()
        rest()
    }

    override var intrinsicContentSize: CGSize {
        CGSize(width: Self.width, height: Self.maxHeight)
    }

    /// A frame. Clamped because a bar is a drawn rectangle: a level outside
    /// 0...1 would become a capsule with a negative height rather than a wrong
    /// one.
    func apply(_ bands: [Double]) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        for (bar, band) in zip(bars, bands) {
            let height = Self.minHeight + band.clamped() * (Self.maxHeight - Self.minHeight)
            bar.bounds = CGRect(x: 0, y: 0, width: Self.barWidth, height: height)
        }
        CATransaction.commit()
    }

    /// Hold still.
    func rest() { apply(Self.resting) }

    override func appearanceChanged() { paint() }

    /// Gay mode spreads the bars a third of the flag apart, moving round it
    /// with the accent; on a selected row they stay the row's white. Every
    /// step of the rainbow is a new tint, which is what repaints them.
    private func paint() {
        let colour = resolved(tint)
        for (index, bar) in bars.enumerated() {
            bar.backgroundColor = Rainbow.drawn && tint != .white
                ? resolved(PlatformColor(KoanAccent.rainbow(Rainbow.step + index * 4).color))
                : colour
        }
    }
}
