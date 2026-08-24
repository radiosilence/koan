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
struct ArtworkBleed: View {
    /// Nothing playing, or a record with no art, means no wash rather than a
    /// grey one.
    let source: AlbumArtwork.Source?
    /// Whether the wash drifts. The room breathes while something is playing
    /// and settles when it stops.
    var drifts = false

    @Environment(CoverArtCache.self) private var cache
    /// Held rather than drawn through `AlbumArtwork`, which shows a placeholder
    /// while it loads. Over a five second dissolve that placeholder is a long
    /// grey wipe between two records, so the old cover stays up until the new
    /// one is actually in hand.
    @State private var image: NSImage?
    /// Bumped when the cover changes, which is what the dissolve keys on. The
    /// image itself cannot: `NSImage` is a reference and identity is not enough
    /// to drive a transition.
    @State private var generation = 0

    /// The cover is blurred to mush, so it is rendered small and scaled up
    /// afterwards. Blurring a 360pt layer and magnifying the result costs a
    /// fraction of blurring one the width of the window, which matters when it
    /// is redrawn twenty times a second.
    private static let side: CGFloat = 360

    var body: some View {
        GeometryReader { geo in
            // Paused rather than switched off, so stopping playback leaves the
            // drift where it is instead of snapping it back.
            TimelineView(.animation(minimumInterval: 1 / 20, paused: !drifts)) { context in
                ZStack {
                    if let image {
                        // Three incommensurate periods, so the drift never
                        // arrives back where it started and never reads as a
                        // loop.
                        let t = context.date.timeIntervalSinceReferenceDate
                        Image(nsImage: image)
                            .resizable()
                            .aspectRatio(contentMode: .fill)
                            .frame(width: Self.side, height: Self.side)
                            .blur(radius: 14)
                            .scaleEffect(geo.size.width / Self.side * (1.25 + 0.06 * sin(t / 13)))
                            .rotationEffect(.degrees(3 * sin(t / 19)))
                            .offset(
                                x: geo.size.width * 0.04 * sin(t / 23),
                                y: geo.size.height * 0.06 * cos(t / 17)
                            )
                            .saturation(1.6)
                            .id(generation)
                            .transition(.opacity)
                    }
                }
                .frame(width: geo.size.width, height: geo.size.height)
            }
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
        .animation(.easeInOut(duration: 3), value: generation)
        .task(id: source) { await load() }
    }

    private func load() async {
        guard let source else {
            show(nil)
            return
        }
        if let cached = cache.cached(source) {
            show(cached)
            return
        }
        let loaded = await cache.image(for: source)
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
