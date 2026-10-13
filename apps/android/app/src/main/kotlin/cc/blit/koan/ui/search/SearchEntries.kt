package cc.blit.koan.ui.search

import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.ui.navigation.Tab

/**
 * Search's own page. Its sections' "See all" open the library's browsers on
 * `Shelf.Search`, which are the listings the sections are the head of.
 */
fun EntryProviderScope<NavKey>.searchEntries() {
    entry<Tab.Search> { SearchPage() }
}
