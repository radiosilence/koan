import SwiftUI

/// The heart, wherever something can be favourited.
///
/// One view so a track row, a queue row, an album and an artist all behave the
/// same: filled and red when on, and otherwise only visible on hover so a list
/// of forty tracks is not a column of grey hearts.
struct FavouriteButton: View {
    let isOn: Bool
    /// Whether to show it while it is off — hover, usually.
    var showing: Bool = true
    var size: Font = .body
    /// A shortcut to mention in the tooltip, where one reaches this heart.
    var hint: String?
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: isOn ? "heart.fill" : "heart")
                .font(size)
                .foregroundStyle(isOn ? AnyShapeStyle(.red) : AnyShapeStyle(.tertiary))
                .contentTransition(.symbolEffect(.replace))
        }
        .buttonStyle(.plain)
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
// carries them, one heart flipping re-ran every visible row on screen: the set
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

    @Environment(LibraryModel.self) private var library

    var body: some View {
        FavouriteButton(isOn: library.isFavourite(album: albumId), showing: showing, size: size) {
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
