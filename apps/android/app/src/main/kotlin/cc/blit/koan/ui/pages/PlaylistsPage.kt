package cc.blit.koan.ui.pages

import androidx.compose.runtime.Composable
import cc.blit.koan.engine.KoanRuntime

/** The list of playlists (Route.Playlists). */
@Composable
fun PlaylistsPage(runtime: KoanRuntime) {
    PlaceholderPage("playlists", "runtime.playlists.playlists and covers; create, rename, delete, reorderPlaylists; naming.")
}
