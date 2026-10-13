package cc.blit.koan.ui.search

import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.ui.navigation.Tab
import cc.blit.koan.ui.navigation.nonce
import kotlinx.serialization.Serializable

/** The three listings a search's sections open: all of each kind the query finds. */
enum class SearchList { Artists, Albums, Tracks }

/** A section's "See all": the whole listing behind it, in the search's order. */
@Serializable
data class SearchListRoute(val list: SearchList, val query: String, val nonce: String = nonce()) : NavKey

fun EntryProviderScope<NavKey>.searchEntries() {
    entry<Tab.Search> { SearchPage() }
    entry<SearchListRoute> { key -> SearchListPage(key.list, key.query) }
}
