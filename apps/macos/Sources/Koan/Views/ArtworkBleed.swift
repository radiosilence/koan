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

    var body: some View {
        GeometryReader { geo in
            if let source {
                // Square art drawn at the full width and centred vertically:
                // the wash wants the middle of the cover, not a letterboxed
                // copy of the whole thing.
                AlbumArtwork(source: source, cornerRadius: 0)
                    .frame(width: geo.size.width, height: geo.size.width)
                    .offset(y: (geo.size.height - geo.size.width) / 2)
                    .blur(radius: 64)
                    .saturation(1.6)
                    .id(source)
                    .transition(.opacity)
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
        // One record dissolving into the next. Long enough to read as the room
        // changing colour rather than as a redraw.
        .animation(.smooth(duration: 0.55), value: source)
    }
}
