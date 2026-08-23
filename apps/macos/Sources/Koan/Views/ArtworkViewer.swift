import KoanFFI
import SwiftUI

/// The cover, as large as the window allows.
///
/// A sheet rather than a window: it belongs to what you were looking at, and
/// dismisses with Escape or a click anywhere — nobody wants to hunt for a close
/// button on a picture.
struct ArtworkViewer: View {
    let source: AlbumArtwork.Source
    let title: String
    let subtitle: String?

    @Environment(\.dismiss) private var dismiss

    var body: some View {
        ZStack {
            // Fills the sheet so a click anywhere outside the cover dismisses.
            Color.black.opacity(0.001)
                .contentShape(.rect)
                .onTapGesture { dismiss() }

            VStack(spacing: 16) {
                // Fills whatever the window allows, less a margin so the cover
                // never runs to the edge. Capping it at a fixed size made a
                // 4000px scan no bigger than a thumbnail on a large display,
                // which defeats the point of opening it.
                AlbumArtwork(source: source, cornerRadius: 10)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .shadow(color: .black.opacity(0.45), radius: 32, y: 14)
                    .onTapGesture { dismiss() }

                VStack(spacing: 3) {
                    Text(title)
                        .font(.title3.weight(.semibold))
                        .multilineTextAlignment(.center)
                    if let subtitle {
                        Text(subtitle)
                            .font(.callout)
                            .foregroundStyle(.secondary)
                            .multilineTextAlignment(.center)
                    }
                }
                .padding(.horizontal, 24)
            }
            .padding(44)
        }
        // Ideal rather than fixed: a sheet is bounded by its window, so this
        // asks for a large one and takes what it is given.
        .frame(
            minWidth: 420,
            idealWidth: 1100,
            maxWidth: .infinity,
            minHeight: 420,
            idealHeight: 1100,
            maxHeight: .infinity
        )
        // Escape, without a visible button cluttering the picture.
        .onExitCommand { dismiss() }
    }
}

/// Makes any artwork open the viewer when clicked.
///
/// A modifier rather than a wrapper view so the album page, the transport bar
/// and anywhere else showing a cover all behave the same without each
/// remembering to.
struct ArtworkPresenter: ViewModifier {
    let source: AlbumArtwork.Source
    let title: String
    let subtitle: String?

    @State private var showing = false

    func body(content: Content) -> some View {
        content
            .onTapGesture { showing = true }
            .help("Show artwork")
            .sheet(isPresented: $showing) {
                ArtworkViewer(source: source, title: title, subtitle: subtitle)
            }
    }
}

extension View {
    func showsArtworkFullSize(
        source: AlbumArtwork.Source,
        title: String,
        subtitle: String? = nil
    ) -> some View {
        modifier(ArtworkPresenter(source: source, title: title, subtitle: subtitle))
    }
}
