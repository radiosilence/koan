package cc.blit.koan.ui.pages

import androidx.compose.runtime.Composable
import cc.blit.koan.engine.KoanRuntime

/** One playlist, laid out like the queue. */
@Composable
fun PlaylistPage(runtime: KoanRuntime, playlistId: Long) {
    PlaceholderPage("playlist", "runtime.playlists.entries (loaded before the move), fillable(id), reorder, remove.")
}
