import KoanFFI
import SwiftUI

/// An artist as a tappable chip. Used for search results and for the similar
/// artists on an artist page — same look, same behaviour, one definition.
struct ArtistPill: View {
    let name: String
    let artistId: Int64
    /// Similar-artist chips carry a score; search results don't.
    var detail: String?

    @Environment(LibraryModel.self) private var library
    @State private var hovering = false

    var body: some View {
        Button {
            library.reveal(artist: artistId)
        } label: {
            HStack(spacing: 5) {
                Image(systemName: "music.mic")
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
                Text(name)
                    .font(.callout)
                if let detail {
                    Text(detail)
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                }
            }
            .padding(.horizontal, 11)
            .padding(.vertical, 6)
            .background(hovering ? AnyShapeStyle(.tertiary) : AnyShapeStyle(.quaternary), in: Capsule())
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help("Go to \(name)")
    }
}
