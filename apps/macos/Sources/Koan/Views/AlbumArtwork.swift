import KoanFFI
import SwiftUI

/// Square album art with a placeholder that doesn't look broken while loading.
struct AlbumArtwork: View {
    enum Source: Hashable {
        case album(Int64)
        case track(Int64)
    }

    let source: Source
    var cornerRadius: CGFloat = 6

    @Environment(CoverArtCache.self) private var cache

    /// Held per view rather than read from the cache during `body`.
    ///
    /// Reading the cache's dictionaries from `body` made every artwork observe
    /// every entry, so one cover arriving invalidated all of them — and the
    /// lookup also inserted into the in-flight set while rendering, which
    /// invalidated them again. Scrolling the album grid pinned ten cores.
    @State private var image: NSImage?
    @State private var isLoading = false

    var body: some View {
        // A square Color drives the layout and the image sits in an overlay, so
        // a non-square cover can't stretch the cell it lives in. Sizing the
        // container from the image instead lets a wide cover push into its
        // neighbours, which is what it was doing.
        Color.clear
            .aspectRatio(1, contentMode: .fit)
            .overlay {
                if let image {
                    Image(nsImage: image)
                        .resizable()
                        .interpolation(.high)
                        .aspectRatio(contentMode: .fill)
                } else {
                    Rectangle()
                        .fill(.quaternary)
                        .overlay {
                            if isLoading {
                                ProgressView().controlSize(.small)
                            } else {
                                EnsoPlaceholder()
                            }
                        }
                }
            }
            .clipShape(RoundedRectangle(cornerRadius: cornerRadius))
            .overlay {
                RoundedRectangle(cornerRadius: cornerRadius)
                    .strokeBorder(.white.opacity(0.06))
            }
            // Keyed on the source, so a recycled cell in a scrolling grid
            // cancels the load it no longer needs and starts the one it does.
            .task(id: source) {
                image = cache.cached(source)
                guard image == nil else { return }

                // Settle first. `.task(id:)` is cancelled when the cell is
                // recycled, so flying past a cover never starts its fetch —
                // without this, one flick through the grid queued a request for
                // every album in the library.
                try? await Task.sleep(for: .milliseconds(180))
                guard !Task.isCancelled else { return }

                isLoading = true
                image = await cache.image(for: source)
                isLoading = false
            }
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
