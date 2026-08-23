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
                            Image(systemName: "music.note")
                                .font(.system(size: 20, weight: .light))
                                .foregroundStyle(.tertiary)
                        }
                }
            }
            .clipShape(RoundedRectangle(cornerRadius: cornerRadius))
            .overlay {
                RoundedRectangle(cornerRadius: cornerRadius)
                    .strokeBorder(.white.opacity(0.06))
            }
    }

    private var image: NSImage? {
        switch source {
        case .album(let id): cache.art(albumId: id)
        case .track(let id): cache.art(trackId: id)
        }
    }
}
