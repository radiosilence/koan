import SwiftUI

/// A playlist's face: the first four records in it, in a 2×2 grid.
///
/// Fewer than four is not padded out with blanks — one record fills the square,
/// two split it down the middle, three leave the fourth quarter to the third.
/// A playlist with nothing in it gets the same ensō placeholder a record with no
/// sleeve does, so an empty playlist reads as empty rather than as broken.
struct PlaylistArtwork: View {
    let sources: [AlbumArtwork.Source]
    var cornerRadius: CGFloat = 6

    var body: some View {
        Color.clear
            .aspectRatio(1, contentMode: .fit)
            .overlay { mosaic }
            .clipShape(RoundedRectangle(cornerRadius: cornerRadius))
            .overlay {
                RoundedRectangle(cornerRadius: cornerRadius)
                    .strokeBorder(.white.opacity(0.06))
            }
    }

    @ViewBuilder
    private var mosaic: some View {
        switch sources.count {
        case 0:
            Rectangle().fill(.quaternary).overlay { EnsoPlaceholder() }
        case 1:
            tile(sources[0])
        case 2:
            // Side by side rather than stacked: sleeves are square, and two
            // half-height strips would crop both to letterboxes.
            HStack(spacing: 0) {
                tile(sources[0])
                tile(sources[1])
            }
        case 3:
            VStack(spacing: 0) {
                HStack(spacing: 0) {
                    tile(sources[0])
                    tile(sources[1])
                }
                // The third spans the bottom, so no quarter is left empty.
                tile(sources[2])
            }
        default:
            VStack(spacing: 0) {
                HStack(spacing: 0) {
                    tile(sources[0])
                    tile(sources[1])
                }
                HStack(spacing: 0) {
                    tile(sources[2])
                    tile(sources[3])
                }
            }
        }
    }

    /// No corner radius and no border on the pieces — the mosaic is one square
    /// with one edge, not four little squares in a box.
    private func tile(_ source: AlbumArtwork.Source) -> some View {
        AlbumArtwork(source: source, cornerRadius: 0, fills: true)
    }
}
