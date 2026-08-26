import AppKit
import SwiftUI

/// The cover, blurred out to a wash of the record's colour behind a header.
///
/// `backgroundExtensionEffect` is what makes it worth doing: it mirrors the
/// blur outwards into the insets around the detail column, so the colour
/// carries under the glass sidebar and toolbar instead of stopping at a hard
/// edge where the pane begins. The gradient goes on first, so the mirrored copy
/// fades out the same way the real one does.
///
/// Fills whatever it is given — a `.background` on a header takes the header's
/// height; anywhere else, say how far down the page the wash should reach.
///
/// There is one of these in the app, on the window. A second copy on the page
/// bought nothing — it sat on the page's own opaque ground, hiding the window's
/// and animating alongside it.
struct ArtworkBleed: View {
    /// Nothing playing, or a record with no art, means no wash rather than a
    /// grey one.
    let source: AlbumArtwork.Source?
    /// Whether there is anything to breathe to. The room breathes while
    /// something is playing and settles when it stops.
    var drifts = false

    @Environment(CoverArtCache.self) private var cache
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @AppStorage("graphics") private var graphics = Graphics.full
    /// Held rather than drawn through `AlbumArtwork`, which shows a placeholder
    /// while it loads. Over a five second dissolve that placeholder is a long
    /// grey wipe between two records, so the old cover stays up until the new
    /// one is actually in hand.
    @State private var image: NSImage?
    /// Bumped when the cover changes, which is what the dissolve keys on. The
    /// image itself cannot: `NSImage` is a reference and identity is not enough
    /// to drive a transition.
    @State private var generation = 0
    /// Both ends of the drift. Flipped once, then left alone — the animations
    /// below repeat forever off it, which is what keeps this off the main
    /// thread.
    @State private var drifted = false

    /// The cover is blurred to mush, so it is rendered small and scaled up
    /// afterwards. Blurring a 360pt layer and magnifying the result costs a
    /// fraction of blurring one the width of the window.
    private static let side: CGFloat = 360

    /// How far the drift travels, and the overscan that lets it.
    ///
    /// What decides whether motion is visible here is not its speed but how far
    /// it goes against how soft the thing moving is. Blurred at 14 points and
    /// magnified about five times, the wash has no feature narrower than eighty
    /// points on screen — so travel has to be read in multiples of that, not in
    /// points. The excursion this file shipped with moved it about one and a
    /// half of those over twenty-three seconds, which is below the threshold at
    /// which slow movement registers at all: it was running, and costing, and
    /// nobody ever saw it.
    ///
    /// These reach about four. Same periods, same easing, same unhurried thing
    /// — it simply arrives somewhere.
    ///
    /// `near` is a floor, not a taste: at full reach the offset carries the
    /// texture 12% of the window sideways and the rotation eats about another
    /// 3.5%, and the scale has to be wide enough that its edge stays out of
    /// frame throughout.
    private static let near: CGFloat = 1.38
    private static let far: CGFloat = 1.58
    private static let reach: CGFloat = 0.12
    private static let rise: CGFloat = 0.10

    /// Whether the wash is actually moving: something to breathe to, a setting
    /// that allows it, and a system that has not asked for less motion.
    private var breathes: Bool { drifts && graphics.drifts && !reduceMotion }

    /// Three incommensurate periods, so the drift never arrives back where it
    /// started and never reads as a loop. Settling is a plain ease: playback
    /// stopping should let the room come to rest, not stop it mid-breath.
    private func drift(_ period: Double) -> Animation {
        breathes
            ? .easeInOut(duration: period).repeatForever(autoreverses: true)
            : .easeInOut(duration: 2)
    }

    var body: some View {
        // Below `reduced` this is nothing at all rather than a transparent
        // wash: no cover fetched, no blur, no mirrored copy under the glass.
        if graphics.showsWash {
            bleed
        }
    }

    private var bleed: some View {
        GeometryReader { geo in
            // The transforms live on this container rather than on the image,
            // so a record change swaps what is inside without interrupting the
            // drift. On the image they went with it, and each new cover
            // arrived parked at the end of a motion that never restarted.
            ZStack {
                if let image {
                    Image(nsImage: image)
                        .resizable()
                        .aspectRatio(contentMode: .fill)
                        .id(generation)
                        .transition(.opacity)
                }
            }
            .frame(width: Self.side, height: Self.side)
            .blur(radius: 14)
            .saturation(1.6)
            // Rasterised here, once, and magnified as a texture from this point
            // down. Without it the blur and the saturation were recomputed on
            // every frame of the drift below — the transforms were never the
            // expensive part, re-blurring behind them was, and it cost about a
            // tenth of a core for as long as anything was playing. The wash is
            // already blurred to mush at 360 points, so there is no detail left
            // for the magnification to lose.
            .drawingGroup()
            // Driven by animation rather than by a `TimelineView` re-rendering
            // at 20fps. Every tick of that invalidated layout through the
            // geometry modifiers below, and a full window Auto Layout pass
            // twenty times a second was a quarter of a core with nothing
            // happening. These hand the interpolation to CoreAnimation, which
            // runs them off the main thread and smoother for it.
            //
            // Scaled off the longer edge rather than the width. The texture is
            // square, so on a window taller than it is wide the width alone
            // does not cover it, and the excursions below would swing the
            // texture's own edge into frame.
            .scaleEffect(
                max(geo.size.width, geo.size.height) / Self.side
                    * (drifted ? Self.far : Self.near)
            )
            .animation(drift(13), value: drifted)
            .rotationEffect(.degrees(drifted ? 3 : -3))
            .animation(drift(19), value: drifted)
            .offset(
                x: geo.size.width * (drifted ? Self.reach : -Self.reach),
                y: geo.size.height * (drifted ? -Self.rise : Self.rise)
            )
            .animation(drift(23), value: drifted)
            .frame(width: geo.size.width, height: geo.size.height)
        }
        .clipped()
        .opacity(0.5)
        .mask(
            LinearGradient(
                colors: [.black, .black, .clear],
                startPoint: .top,
                endPoint: .bottom
            )
        )
        .backgroundExtensionEffect()
        .allowsHitTesting(false)
        // One record dissolving into the next over long enough that you notice
        // the room has changed colour without ever catching it changing.
        .animation(.easeInOut(duration: 2), value: generation)
        .task(id: source) { await load() }
        .onAppear { drifted = breathes }
        .onChange(of: breathes) { _, now in drifted = now }
    }

    private func load() async {
        guard let source else {
            show(nil)
            return
        }
        if let cached = cache.cached(source, size: .tile) {
            show(cached)
            return
        }
        let loaded = await cache.image(for: source, size: .tile)
        // A cancelled load means the record moved on again; whatever came back
        // is for the wrong one.
        guard !Task.isCancelled else { return }
        show(loaded)
    }

    private func show(_ new: NSImage?) {
        guard new !== image else { return }
        image = new
        generation += 1
    }
}

extension View {
    /// Stands a list's own ground down so the window's wash shows through it.
    ///
    /// A `List` paints an opaque background by default, which lands on top of
    /// the wash and stops the record's colour in a hard line under the header
    /// instead of letting it fade out across the first few rows. Every list in
    /// the app sits in the wash, so every list gives its ground up.
    func washedGround() -> some View {
        scrollContentBackground(.hidden)
    }
}
