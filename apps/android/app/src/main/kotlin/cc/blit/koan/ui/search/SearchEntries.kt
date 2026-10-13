package cc.blit.koan.ui.search

import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.ui.StubPage
import cc.blit.koan.ui.navigation.Tab

fun EntryProviderScope<NavKey>.searchEntries() {
    entry<Tab.Search> { StubPage("Search") }
}
