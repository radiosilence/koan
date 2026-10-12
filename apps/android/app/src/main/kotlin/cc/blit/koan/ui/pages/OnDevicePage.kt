package cc.blit.koan.ui.pages

import androidx.compose.runtime.Composable
import cc.blit.koan.engine.KoanRuntime

/** The On Device shelf (Section.OnDevice). */
@Composable
fun OnDevicePage(runtime: KoanRuntime) {
    PlaceholderPage("on device", "runtime.library.visibleShelf; library.browse(ShelfList, ShelfKind.Downloaded) for See all.")
}
