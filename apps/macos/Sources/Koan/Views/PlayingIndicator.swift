import AppKit
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
    @AppStorage("graphics") private var graphics = Graphics.full

    /// Whether the bars follow the music. Reduce Motion asks them not to, and
    /// so does the bottom of the graphics ladder; off stage nobody is looking.
    private var live: Bool { onStage && !reduceMotion && graphics.animatesIndicators }

    var body: some View {
        PlayingBars(live: live, tint: NSColor(tint), levels: levels)
            .frame(width: PlayingBarsView.width, height: PlayingBarsView.maxHeight)
            .accessibilityLabel(isPlaying ? "Playing" : "Paused")
    }
}

private struct PlayingBars: NSViewRepresentable {
    let live: Bool
    let tint: NSColor
    let levels: PlayingLevels

    func makeNSView(context: Context) -> PlayingBarsView { PlayingBarsView() }

    func updateNSView(_ view: PlayingBarsView, context: Context) {
        view.tint = tint
        view.source = levels
        if live {
            levels.attach(view)
        } else {
            levels.detach(view)
            view.rest()
        }
    }

    static func dismantleNSView(_ view: PlayingBarsView, coordinator: ()) {
        view.source?.detach(view)
    }
}

/// Three capsules, moved by their bounds. Nothing else about them ever
/// changes, and a bounds change with actions disabled is the cheapest thing a
/// layer can be asked to do.
final class PlayingBarsView: NSView {
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

    var tint: NSColor = .labelColor {
        didSet {
            guard tint != oldValue else { return }
            paint()
        }
    }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        for (index, bar) in bars.enumerated() {
            bar.cornerRadius = Self.barWidth / 2
            // Grown from the foot, so a height is one number rather than a
            // height and a position that must agree.
            bar.anchorPoint = CGPoint(x: 0.5, y: 0)
            bar.position = CGPoint(
                x: Double(index) * (Self.barWidth + Self.spacing) + Self.barWidth / 2,
                y: 0
            )
            bar.actions = ["bounds": NSNull(), "position": NSNull(), "backgroundColor": NSNull()]
            layer?.addSublayer(bar)
        }
        paint()
        rest()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not from a nib") }

    override var intrinsicContentSize: NSSize {
        NSSize(width: Self.width, height: Self.maxHeight)
    }

    /// A frame. Clamped because a bar is a drawn rectangle: the level keeps
    /// itself inside 0...1 today, but one that ever stepped outside it would
    /// become a capsule with a negative height rather than a wrong one.
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

    private func paint() {
        for bar in bars { bar.backgroundColor = tint.cgColor }
    }
}
