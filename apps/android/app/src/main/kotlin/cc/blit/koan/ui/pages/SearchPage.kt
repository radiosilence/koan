package cc.blit.koan.ui.pages

import androidx.compose.runtime.Composable
import cc.blit.koan.engine.KoanRuntime

/** Search (the Search tab's root). */
@Composable
fun SearchPage(runtime: KoanRuntime) {
    PlaceholderPage("search", "Section.SearchResults. There is no SearchModel yet: port SearchModel.swift (the engine's search) as part of this page.")
}
