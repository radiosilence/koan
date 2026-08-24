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
    @Environment(UIState.self) private var ui

    /// Room for the caption below the cover, and the margin around the lot.
    private static let caption: CGFloat = 76
    private static let margin: CGFloat = 48

    /// Covers are square, so the sheet is too — sized off the window it hangs
    /// from rather than its own proposal, which SwiftUI answers with a tall
    /// narrow box that leaves the cover no wider than a thumbnail.
    private var side: CGFloat {
        let host = ui.windowSize
        guard host != .zero else { return 700 }
        let width = host.width - Self.margin * 2
        let height = host.height - Self.margin * 2 - Self.caption
        return max(320, min(width, height))
    }

    var body: some View {
        VStack(spacing: 16) {
            AlbumArtwork(source: source, cornerRadius: 10)
                .frame(width: side, height: side)
                .shadow(color: .black.opacity(0.45), radius: 32, y: 14)

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
            .frame(width: side)
            .frame(height: Self.caption - 16)
        }
        .padding(Self.margin / 2)
        // A click anywhere dismisses; there is no close button on a picture.
        .contentShape(.rect)
        .onTapGesture { dismiss() }
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
