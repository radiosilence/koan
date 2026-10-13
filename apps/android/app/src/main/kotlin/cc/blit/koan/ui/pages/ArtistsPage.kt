package cc.blit.koan.ui.pages

import androidx.compose.runtime.Composable
import cc.blit.koan.engine.KoanRuntime

/** The artist list (Section.Artists). */
@Composable
fun ArtistsPage(runtime: KoanRuntime) {
    PlaceholderPage("artists", "runtime.library.visibleArtists; navigator.openArtist(id).")
}
