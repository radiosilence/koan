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
        ZStack {
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
        .aspectRatio(1, contentMode: .fit)
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
