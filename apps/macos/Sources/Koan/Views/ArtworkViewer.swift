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
    #if os(macOS)
    @Environment(UIState.self) private var ui
    #endif

    /// Room for the caption below the cover, and the margin around the lot.
    private static let caption: CGFloat = 76
    private static let margin: CGFloat = 48

    #if os(macOS)
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
    #endif

    var body: some View {
        VStack(spacing: 16) {
            AlbumArtwork(source: source, size: .full, cornerRadius: KoanTheme.radius(10))
                #if os(macOS)
                .frame(width: side, height: side)
                #else
                // A sheet on a phone is the size of the screen: the largest
                // square that fits in it, the whole cover in view.
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                #endif
                .koanShadow(0.45, radius: 32, y: 14)

            VStack(spacing: 3) {
                Text(title)
                    .koanText(.titleSmall, .strong)
                    .multilineTextAlignment(.center)
                if let subtitle {
                    Text(subtitle)
                        .koanText(.meta, .muted)
                        .multilineTextAlignment(.center)
                }
            }
            #if os(macOS)
            .frame(width: side)
            #else
            .frame(maxWidth: .infinity)
            #endif
            .frame(height: Self.caption - 16)
        }
        .padding(Self.margin / 2)
        .koanSheet()
        // A click anywhere dismisses; there is no close button on a picture.
        .contentShape(.rect)
        .onTapGesture { dismiss() }
        #if os(macOS)
        .onExitCommand { dismiss() }
        #endif
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
