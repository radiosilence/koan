#if canImport(AppKit)
import AppKit
#else
import UIKit
#endif
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
/// The Mac has one of these, on the window; a phone paints one behind each
/// page — see `WashLayer`.
struct ArtworkBleed: View {
    /// Nothing playing, or a record with no art, means no wash rather than a
    /// grey one.
    let source: AlbumArtwork.Source?
    /// Whether there is anything to breathe to. The room breathes while
    /// something is playing and settles when it stops.
    var drifts = false
    /// Whether the app is in front. The rainbow drifts with nothing playing,
    /// but not behind.
    var inFront = true

    @Environment(CoverArtCache.self) private var cache
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @AppStorage("graphics") private var graphics = Graphics.full
    /// The last cover that had to be fetched, and the record it was for. Only
    /// consulted when the cache cannot answer.
    @State private var fetched: (source: AlbumArtwork.Source, image: PlatformImage?)?

    /// What this record's sleeve is — read straight through the cache on every
    /// pass, the way `AlbumArtwork` reads its bitmap. Held in `@State` and
    /// written by a task, the wash would be a second commit after every navigation,
    /// and a commit that dirties a drawn layer is a synchronous round trip to
    /// the render server whatever it is carrying.
    ///
    /// Doubly optional on purpose. The outer `nil` means *nobody has answered
    /// yet*, which is not the same as a record having no cover: the first keeps
    /// the room as it is until the sleeve arrives, the second empties it. Told
    /// apart, a record whose art is still being fetched does not wipe the wash
    /// grey and then fade the new one in over two seconds.
    private var answered: PlatformImage?? {
        if rainbow { return .some(Rainbow.wash) }
        guard let source, !cache.isAbsent(source) else { return .some(nil) }
        if let held = cache.cached(source, size: .tile) { return .some(held) }
        guard let fetched, fetched.source == source else { return nil }
        return .some(fetched.image)
    }

    @Environment(\.powerSaving) private var powerSaving
    /// For gay mode's disco, which pulses with the music. Optional, as the
    /// appearance is: the window's background is handed it explicitly.
    @Environment(PlayingLevels.self) private var levels: PlayingLevels?
    @Environment(\.colorScheme) private var scheme
    /// Optional: the window's background is built outside the environment
    /// the app hands its views, and is given this explicitly.
    @Environment(AppearanceModel.self) private var appearance: AppearanceModel?
    /// Whether the wash is moving: something to breathe to, a setting that
    /// allows it, and a system that has not asked for less motion.
    /// The rainbow drifts whether or not anything plays, while the app is in
    /// front.
    private var breathes: Bool { (drifts || rainbow && inFront) && graphics.drifts && !reduceMotion && !powerSaving }

    /// Gay mode: its sheen in place of the sleeve, whatever the record and
    /// whether or not colours come from it.
    private var rainbow: Bool { appearance?.rainbowDrawn == true }

    var body: some View {
        // Below `reduced` this is nothing at all rather than a transparent
        // wash: no cover fetched, no blur, no mirrored copy under the glass.
        // And nothing when colours from the record are off.
        if graphics.showsWash, appearance?.recordColours != false || rainbow {
            bleed
        }
    }

    /// Everything that moves is in `DriftingWash`, and everything left here is
    /// static — a mask, an opacity and a mirror, committed once. Nothing in
    /// this view is animated: the drift, the blur and the dissolve between
    /// records belong to the compositor, and this view's body runs when a
    /// record changes and at no other time.
    @ViewBuilder
    private var bleed: some View {
        if KoanTheme.isOn {
            // The kōan theme's wash: toned so no text over it loses contrast,
            // at the theme's strength, the whole height of the bare ground.
            // Flat surfaces cover the rest; nothing glass extends it under them.
            DriftingWash(
                image: answered ?? nil,
                pending: answered == nil,
                drifts: breathes,
                tone: scheme == .dark ? .dark : .light
            )
            .overlay { disco }
            .opacity(KoanTheme.wash)
            .allowsHitTesting(false)
            .task(id: source) { await load() }
        } else {
            DriftingWash(image: answered ?? nil, pending: answered == nil, drifts: breathes, tone: systemTone)
                .overlay { disco }
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
    }

    /// Gay mode's disco over its sheen, breathing with the music. Only while
    /// the rainbow is drawn and motion allowed: it is what keeps the analyser
    /// awake.
    @ViewBuilder private var disco: some View {
        if rainbow, breathes, let levels {
            RainbowPulse(levels: levels)
        }
    }

    /// The platform's look holds the wash to the theme's limits on a phone,
    /// where secondary text, track numbers and lengths sit straight on it: a
    /// saturated sleeve otherwise darkens a light page past what they read on.
    /// The Mac's wash fades out under its header and keeps its full colour.
    private var systemTone: WashTone? {
        #if os(iOS)
        scheme == .dark ? .dark : .light
        #else
        nil
        #endif
    }

    /// Only for a cover the cache could not already answer for. The usual path
    /// is read through in `answered`, in the same pass as the page that changed it.
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
        #if os(tvOS)
        // A tvOS list paints no ground of its own; the theme's sets its type.
        koanList()
        #else
        // And in the theme, the theme's list: rows on the ground, ruled.
        scrollContentBackground(.hidden)
            .koanList()
        #endif
    }
}
