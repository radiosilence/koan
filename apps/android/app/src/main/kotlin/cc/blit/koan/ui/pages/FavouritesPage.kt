package cc.blit.koan.ui.pages

import androidx.compose.runtime.Composable
import cc.blit.koan.engine.KoanRuntime

/** The Favourites shelf (Section.Favourites). */
@Composable
fun FavouritesPage(runtime: KoanRuntime) {
    PlaceholderPage("favourites", "runtime.library.visibleShelf; library.browse(ShelfList, ShelfKind.Favourites) for See all.")
}
