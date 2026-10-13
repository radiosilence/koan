package cc.blit.koan.ui.pages

import androidx.compose.runtime.Composable
import cc.blit.koan.engine.KoanRuntime

/** One record page. */
@Composable
fun AlbumPage(runtime: KoanRuntime, albumId: Long) {
    PlaceholderPage("album", "runtime.library.detailRecords[albumId] (loaded before the move), navigator.highlightedTrackId, library.favouriteTrackIds.")
}
