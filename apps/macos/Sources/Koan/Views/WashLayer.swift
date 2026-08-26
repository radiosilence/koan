import SwiftUI

/// The room's colour: the record's sleeve, blurred out behind everything.
///
/// Lifted out of `RootView` so the phone can stand in the same room. What the
/// two shells differ on is where it goes — a window's container background on
/// the Mac, behind the tab content on a phone — not what it is.
struct WashLayer: View {
    @Environment(Navigator.self) private var nav
    @Environment(PlayerModel.self) private var player
    @Environment(PlaylistsModel.self) private var playlists
    @Environment(CoverArtCache.self) private var art

    var body: some View {
        // Over an opaque ground, because this replaces the container's own
        // background rather than sitting on it — a half-transparent wash on its
        // own leaves you looking through the app at whatever is behind it.
        ZStack {
            Rectangle().fill(.background)
            ArtworkBleed(source: source, drifts: player.isPlaying)
                .environment(art)
        }
    }

    /// A page about one record answers with it: an album with its own sleeve, a
    /// playlist with the first of its records. Every other page — a grid, a
    /// list of artists, favourites, history — is not about any record in
    /// particular, so it answers with the one playing. The room is coloured by
    /// the music wherever you have wandered off to, and only a page that
    /// disagrees says otherwise.
    private var source: AlbumArtwork.Source? {
        switch nav.current {
        case .album(let id): .album(id)
        case .section(.playlist(let id)): playlists.covers[id]?.first ?? player.currentArtwork
        default: player.currentArtwork
        }
    }
}
