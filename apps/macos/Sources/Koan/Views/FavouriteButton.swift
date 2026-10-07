import SwiftUI

/// The heart, wherever something can be favourited.
///
/// One view so a track row, a queue row, an album and an artist all behave the
/// same: filled and red when on, and otherwise only visible on hover so a list
/// of forty tracks is not a column of grey hearts.
struct FavouriteButton: View {
    let isOn: Bool
    /// Whether to show it while it is off — hover, usually.
    var showing = true
    var size: Font = .body
    /// A shortcut to mention in the tooltip, where one reaches this heart.
    var hint: String?
    /// Room around the glyph that is part of the button, for a host that
    /// draws a ground behind it.
    var inset: CGFloat = 0
    let action: () -> Void

    /// Gay mode: a favourite's heart is the palette, top to bottom.
    @Environment(\.koanRainbow) private var rainbow

    var body: some View {
        Button(action: action) {
            Image(systemName: isOn ? "heart.fill" : "heart")
                .font(size)
                .foregroundStyle(
                    isOn
                        ? (rainbow ? KoanTheme.marker(rainbow: true, vertical: true) : KoanTheme.style(.bad, system: .red))
                        : KoanTheme.style(.muted, system: .tertiary)
                )
                .contentTransition(.symbolEffect(.replace))
                // A little jump on every change, and on a phone a tap in the
                // hand when something becomes a favourite.
                .symbolEffect(.bounce.up.byLayer, options: .speed(1.4), value: isOn)
                .padding(inset)
                .touchTarget()
                // The glyph's outline is hollow; the whole cell takes the click.
                .contentShape(Rectangle())
        }
        .controlButton()
        #if os(iOS)
        .sensoryFeedback(trigger: isOn) { _, on in on ? .impact(weight: .light) : nil }
        #endif
        .opacity(isOn || showing ? 1 : 0)
        .help(help)
        .accessibilityLabel(isOn ? "Remove favourite" : "Favourite")
    }

    private var help: String {
        let verb = isOn ? "Remove favourite" : "Favourite"
        return hint.map { "\(verb) (\($0))" } ?? verb
    }
}

// The hearts below read the favourite sets themselves. Read in the row that
// carries them, one heart flipping would re-run every visible row: the set
// is one property, and a row reading it is a row subscribed to all of it. In
// the leaf, a flip re-runs the hearts and nothing else.

/// The heart for one track.
struct TrackHeart: View {
    let trackId: Int64
    var showing = true
    var size: Font = .body
    var hint: String?

    @Environment(LibraryModel.self) private var library

    var body: some View {
        FavouriteButton(
            isOn: library.isFavourite(track: trackId), showing: showing, size: size, hint: hint
        ) {
            library.toggleFavourite(track: trackId)
        }
    }
}

/// The heart for one record.
struct AlbumHeart: View {
    let albumId: Int64
    var showing = true
    var size: Font = .body
    var inset: CGFloat = 0

    @Environment(LibraryModel.self) private var library

    var body: some View {
        FavouriteButton(isOn: library.isFavourite(album: albumId), showing: showing, size: size, inset: inset) {
            library.toggleFavourite(album: albumId)
        }
    }
}

/// The heart for one artist.
struct ArtistHeart: View {
    let artistId: Int64
    var showing = true
    var size: Font = .body

    @Environment(LibraryModel.self) private var library

    var body: some View {
        FavouriteButton(isOn: library.isFavourite(artist: artistId), showing: showing, size: size) {
            library.toggleFavourite(artist: artistId)
        }
    }
}
