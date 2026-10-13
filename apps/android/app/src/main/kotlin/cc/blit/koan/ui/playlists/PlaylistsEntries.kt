package cc.blit.koan.ui.playlists

import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.ui.StubPage
import cc.blit.koan.ui.navigation.nonce
import kotlinx.serialization.Serializable

@Serializable
data class PlaylistsRoute(val nonce: String = nonce()) : NavKey

@Serializable
data class PlaylistRoute(val playlistId: Long, val nonce: String = nonce()) : NavKey

fun EntryProviderScope<NavKey>.playlistsEntries() {
    entry<PlaylistsRoute> { StubPage("Playlists") }
    entry<PlaylistRoute> { StubPage("Playlist") }
}
