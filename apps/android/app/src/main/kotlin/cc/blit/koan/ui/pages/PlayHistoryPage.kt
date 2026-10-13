package cc.blit.koan.ui.pages

import androidx.compose.runtime.Composable
import cc.blit.koan.engine.KoanRuntime

/** Play history, grouped by day (Section.PlayHistory). */
@Composable
fun PlayHistoryPage(runtime: KoanRuntime) {
    PlaceholderPage("history", "runtime.library.visiblePlayHistory, filter; forgetPlays, clearPlayHistory.")
}
