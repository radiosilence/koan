package cc.blit.koan.ui.pages

import androidx.compose.runtime.Composable
import cc.blit.koan.engine.KoanRuntime

/** The Recently Played shelf (Section.RecentlyPlayed). */
@Composable
fun RecentlyPlayedPage(runtime: KoanRuntime) {
    PlaceholderPage("recently played", "runtime.library.visibleShelf; library.browse(ShelfList, ShelfKind.Recent) for See all.")
}
