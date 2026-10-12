package cc.blit.koan.ui.pages

import androidx.compose.runtime.Composable
import cc.blit.koan.engine.KoanRuntime

/** The album grid (Section.Albums). */
@Composable
fun AlbumsPage(runtime: KoanRuntime) {
    PlaceholderPage("albums", "runtime.library.visibleAlbums, filter, albumSort, browseFilter; sleeves through ui/Sleeve; open with navigator.openAlbum(id).")
}
