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
    @Environment(Navigator.self) private var nav
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 5) {
            Image(systemName: "music.mic")
                .font(.caption2)
                .foregroundStyle(.tertiary)
            // A classical release credits the soloist, the orchestra and the
            // conductor in one artist string, which as a pill is a paragraph
            // laid on its side. The full name is in the tooltip and on the
            // artist page.
            Text(name)
                .font(.callout)
                .lineLimit(1)
                .truncationMode(.tail)
            if let detail {
                Text(detail)
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
            }
        }
        .frame(maxWidth: 260, alignment: .leading)
        .padding(.horizontal, 11)
        .padding(.vertical, 6)
        .fixedSize(horizontal: true, vertical: false)
        .background(hovering ? AnyShapeStyle(.tertiary) : AnyShapeStyle(.quaternary), in: Capsule())
        .contentShape(Capsule())
        .onHover { hovering = $0 }
        .onTapGesture { nav.open(artist: artistId) }
        .help("Go to \(name)")
        .contextMenu { PlayableMenu(playable: .artist(id: artistId, name: name)) }
        .draggablePlayable(.artist(id: artistId, name: name))
    }
}
