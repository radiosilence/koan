package cc.blit.koan.ui.pages

import androidx.compose.runtime.Composable
import cc.blit.koan.engine.KoanRuntime

/** One artist page. */
@Composable
fun ArtistPage(runtime: KoanRuntime, artistId: Long) {
    PlaceholderPage("artist", "runtime.library.detailArtist (albums, appearances, bio info).")
}
