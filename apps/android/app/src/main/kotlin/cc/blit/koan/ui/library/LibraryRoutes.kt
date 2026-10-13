package cc.blit.koan.ui.library

import androidx.navigation3.runtime.NavKey
import cc.blit.koan.ffi.ShelfKind
import cc.blit.koan.ui.navigation.nonce
import kotlinx.serialization.Serializable

@Serializable
data class AlbumRoute(val albumId: Long, val nonce: String = nonce()) : NavKey

@Serializable
data class ArtistRoute(val artistId: Long, val nonce: String = nonce()) : NavKey

/**
 * A question the library answers with artists, records and tracks: what is
 * favourited, played lately, on this device, or matches a search. As
 * `koan_core::shelves` defines them.
 */
@Serializable
sealed interface Shelf {
    @Serializable data object Favourites : Shelf

    @Serializable data object Recent : Shelf

    @Serializable data object Downloaded : Shelf

    @Serializable data class Search(val query: String) : Shelf

    val kind: ShelfKind
        get() = when (this) {
            Favourites -> ShelfKind.Favourites
            Recent -> ShelfKind.Recent
            Downloaded -> ShelfKind.Downloaded
            is Search -> ShelfKind.Search(query)
        }

    val title: String
        get() = when (this) {
            Favourites -> "Favourites"
            Recent -> "Recently Played"
            Downloaded -> "Downloaded"
            is Search -> query
        }
}

/** A shelf's page: the first few of each kind, each opening its browser filtered to the shelf. */
@Serializable
data class ShelfRoute(val shelf: Shelf, val nonce: String = nonce()) : NavKey

/** The album browser, over the whole library or, from a shelf's "See all", filtered to it. */
@Serializable
data class AlbumsRoute(val shelf: Shelf? = null, val nonce: String = nonce()) : NavKey

@Serializable
data class ArtistsRoute(val shelf: Shelf? = null, val nonce: String = nonce()) : NavKey

/**
 * The track browser. Reached only from a shelf: the whole library as one list
 * is tens of thousands of rows to play from at once.
 */
@Serializable
data class TracksRoute(val shelf: Shelf, val nonce: String = nonce()) : NavKey
