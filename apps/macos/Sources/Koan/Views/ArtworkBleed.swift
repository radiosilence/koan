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
    /// The last cover that had to be fetched, and the record it was for. Only
    /// consulted when the cache cannot answer.
    @State private var fetched: (source: AlbumArtwork.Source, image: NSImage?)?

    /// What this record's sleeve is — read straight through the cache on every
    /// pass, the way `AlbumArtwork` reads its bitmap. Held in `@State` and
    /// written by a task, the wash was a second commit after every navigation,
    /// and a commit that dirties a drawn layer is a synchronous round trip to
    /// the render server whatever it is carrying.
    ///
    /// Doubly optional on purpose. The outer `nil` means *nobody has answered
    /// yet*, which is not the same as a record having no cover: the first keeps
    /// the room as it is until the sleeve arrives, the second empties it. Told
    /// apart, a record whose art is still being fetched no longer wipes the
    /// wash grey and then fades the new one in over two seconds.
    private var answered: NSImage?? {
        guard let source else { return .some(nil) }
        if let held = cache.cached(source, size: .tile) { return .some(held) }
        guard let fetched, fetched.source == source else { return nil }
        return .some(fetched.image)
    }

    /// Whether the wash is actually moving: something to breathe to, a setting
    /// that allows it, and a system that has not asked for less motion.
    private var breathes: Bool { drifts && graphics.drifts && !reduceMotion }

    var body: some View {
        // Below `reduced` this is nothing at all rather than a transparent
        // wash: no cover fetched, no blur, no mirrored copy under the glass.
        if graphics.showsWash {
            bleed
        }
    }

    /// Everything that moves is in `DriftingWash`, and everything left here is
    /// static — a mask, an opacity and a mirror, committed once. Nothing in
    /// this view is animated, which is the whole point: the drift, the blur and
    /// the dissolve between records all belong to the compositor now, and this
    /// view's body runs when a record changes and at no other time.
    private var bleed: some View {
        DriftingWash(image: answered ?? nil, pending: answered == nil, drifts: breathes)
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
            .task(id: source) { await load() }
    }

    /// Only for a cover the cache could not already answer for. The usual path
    /// is read through in `cover`, in the same pass as the page that changed it.
    private func load() async {
        guard let source, cache.cached(source, size: .tile) == nil else { return }
        let loaded = await cache.image(for: source, size: .tile)
        // A cancelled load means the record moved on again; whatever came back
        // is for the wrong one.
        guard !Task.isCancelled else { return }
        fetched = (source, loaded)
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
