package cc.blit.koan.ui.library

import androidx.navigation3.runtime.NavKey
import cc.blit.koan.ui.navigation.nonce
import kotlinx.serialization.Serializable

@Serializable
data class AlbumRoute(val albumId: Long, val nonce: String = nonce()) : NavKey

@Serializable
data class ArtistRoute(val artistId: Long, val nonce: String = nonce()) : NavKey
