import KoanFFI
import SwiftUI

/// Square album art with a placeholder that doesn't look broken while loading.
struct AlbumArtwork: View {
    enum Source {
        case album(Int64)
        case track(Int64)
    }

    let source: Source
    var cornerRadius: CGFloat = 6

    @Environment(CoverArtCache.self) private var cache

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
    }

    private var isLoading: Bool {
        switch source {
        case .album(let id): cache.isLoading(albumId: id)
        case .track(let id): cache.isLoading(trackId: id)
        }
    }

    private var image: NSImage? {
        switch source {
        case .album(let id): cache.art(albumId: id)
        case .track(let id): cache.art(trackId: id)
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
                        // Proportional to the icon's own 40/512 stroke.
                        lineWidth: side * 0.078,
                        lineCap: .round,
                        lineJoin: .round
                    )
                )
                .padding(side * 0.2)
        }
        .opacity(0.5)
    }
}
