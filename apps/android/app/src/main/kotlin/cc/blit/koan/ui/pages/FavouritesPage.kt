package cc.blit.koan.ui.pages

import androidx.compose.runtime.Composable
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.nav.Section
import uniffi.koan_ffi.ShelfKind

/** The Favourites shelf (Section.Favourites): everything you hearted, artists, records and tracks in one page. */
@Composable
fun FavouritesPage(runtime: KoanRuntime) {
    ShelfPage(
        runtime, Section.Favourites, ShelfKind.Favourites, "favourites",
        emptyIcon = "heart",
        emptyTitle = "nothing favourited yet",
        emptyDetail = "Hit the heart on a track, a record or an artist.",
    )
}
