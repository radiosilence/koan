import KoanFFI
import SwiftUI

/// Square album art with a placeholder that doesn't look broken while loading.
struct AlbumArtwork: View {
    /// Which record's sleeve. An album wherever the caller knows one: art is
    /// stored per record, so asking by track only to draw the same image is a
    /// round trip and a cache entry per track on it.
    enum Source: Hashable {
        case album(Int64)
        case track(Int64)
    }

    /// How large a bitmap to keep, not how large to draw it.
    ///
    /// Two cached sizes rather than one per call site: every entry is held
    /// until evicted, and a bucket the whole app shares is one bitmap where a
    /// per-frame size would be a dozen near-identical ones. Both are generous
    /// enough for their callers at 2×.
    enum Size: String, Hashable {
        /// List rows, headers and the transport — nothing above 64pt.
        case thumb
        /// Grid tiles, a record's own page, the window wash.
        case tile
        /// The viewer, and only the viewer. Never cached.
        case full

        var pixels: Int? {
            switch self {
            case .thumb: 128
            case .tile: 512
            case .full: nil
            }
        }
    }

    let source: Source
    var size: Size = .tile
    var cornerRadius: CGFloat = 6
    /// Fill whatever frame it is handed rather than sizing itself square. The
    /// pieces of a playlist's 2×2 mosaic are given rectangles to fill; every
    /// other caller wants the square below.
    var fills = false

    @Environment(CoverArtCache.self) private var cache

    /// Held per view rather than read from the cache during `body`.
    ///
    /// Reading the cache's dictionaries from `body` made every artwork observe
    /// every entry, so one cover arriving invalidated all of them — and the
    /// lookup also inserted into the in-flight set while rendering, which
    /// invalidated them again. Scrolling the album grid pinned ten cores.
    @State private var image: NSImage?
    @State private var isLoading = false

    /// A square Color drives the layout and the image sits in an overlay, so a
    /// non-square cover can't stretch the cell it lives in. Sizing the
    /// container from the image instead lets a wide cover push into its
    /// neighbours, which is what it was doing.
    @ViewBuilder
    private var square: some View {
        if fills {
            Color.clear
        } else {
            Color.clear.aspectRatio(1, contentMode: .fit)
        }
    }

    var body: some View {
        // Read straight through on every pass. A cover the app already holds
        // draws in the *same frame* as the view that asks for it — no
        // placeholder, no fade, no waiting for `.task` to run after the first
        // render. Opening a record from the grid it was already showing used to
        // draw a grey square and then dissolve in a bitmap sitting in memory.
        //
        // Safe now in a way it was not: this used to subscribe every artwork on
        // screen to every entry in the cache, because the cache was observable
        // and the lookup touched its dictionaries. It is neither observable nor
        // main-actor bound any more — this is a lock and a hash lookup.
        let ready = image ?? cache.cached(source, size: size)
        return square
            // The ground stays put. Swapping it for the art meant a *cross*
            // dissolve — the placeholder fading out under an image fading in —
            // so for the length of the fade neither was opaque and the cover
            // read as translucent before snapping solid. The art comes up over
            // a ground that never moves, so the composite is opaque throughout.
            .overlay {
                Rectangle()
                    .fill(.quaternary)
                    .overlay {
                        if ready == nil {
                            if isLoading {
                                ProgressView().controlSize(.small)
                            } else {
                                EnsoPlaceholder()
                            }
                        }
                    }
            }
            .overlay {
                if let ready {
                    Image(nsImage: ready)
                        .resizable()
                        // The bitmap is already sized for where it is drawn, so
                        // there is little left to interpolate and `.high` was
                        // paying for a better answer to an easier question.
                        .interpolation(.medium)
                        .aspectRatio(contentMode: .fill)
                        .transition(.opacity)
                }
            }
            // Covers in the grid land tens of milliseconds apart, and cutting
            // straight from placeholder to art reads as a stutter of pops.
            .animation(.easeOut(duration: 0.2), value: ready == nil)
            .clipShape(RoundedRectangle(cornerRadius: cornerRadius))
            .overlay {
                RoundedRectangle(cornerRadius: cornerRadius)
                    .strokeBorder(.white.opacity(0.06))
            }
            // Keyed on the source, so a recycled cell in a scrolling grid
            // cancels the load it no longer needs and starts the one it does.
            .task(id: Key(source: source, size: size)) {
                image = cache.cached(source, size: size)
                guard image == nil else { return }

                // Settle first. `.task(id:)` is cancelled when the cell is
                // recycled, so flying past a cover never starts its fetch —
                // without this, one flick through the grid queued a request for
                // every album in the library.
                try? await Task.sleep(for: .milliseconds(180))
                guard !Task.isCancelled else { return }

                isLoading = true
                image = await cache.image(for: source, size: size)
                FrameTimer.shared.note("art-\(size.rawValue)")
                isLoading = false
            }
    }

    private struct Key: Equatable {
        let source: Source
        let size: Size
    }
}


/// Our own stand-in when a record has no artwork.
///
/// The app icon's own ensō, faded back — better than the server's placeholder,
/// which is a branded blue vinyl that reads as real art, and better than a
/// music-note glyph, which says nothing about koan.
struct EnsoPlaceholder: View {
    var body: some View {
        GeometryReader { geo in
            let side = min(geo.size.width, geo.size.height)
            EnsoShape()
                .stroke(
                    .tertiary,
                    style: StrokeStyle(
                        // Lighter than the icon's own 40/512 stroke: at
                        // placeholder size that weight reads as a fat ring
                        // rather than a brush mark.
                        lineWidth: side * 0.045,
                        lineCap: .round,
                        lineJoin: .round
                    )
                )
                .padding(side * 0.26)
        }
        .opacity(0.5)
    }
}
