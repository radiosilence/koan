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
        // A plain chip rather than glass. Glass samples what is behind it and
        // adapts its own luminance to stay legible on it — which is right for
        // something floating over content, and wrong for a chip sitting *in*
        // it. On a flat page ground every pill sampled the same colour and they
        // all matched; over the wash they each answer to a different part of
        // it, and a row of them reads as a scatter of half-transparent ones
        // rather than a set. A fixed fill takes its share of the colour behind
        // it without arguing with it.
        .background(.quaternary, in: .capsule)
        .contentShape(Capsule())
        .onTapGesture { nav.open(artist: artistId) }
        .help("Go to \(name)")
        .contextMenu { PlayableMenu(playable: .artist(id: artistId, name: name)) }
        .draggablePlayable(.artist(id: artistId, name: name))
    }
}
