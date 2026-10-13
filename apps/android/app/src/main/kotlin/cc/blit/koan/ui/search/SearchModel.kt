package cc.blit.koan.ui.search

import cc.blit.koan.ui.library.Selection
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import cc.blit.koan.ffi.ShelfSummary
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.FlowPreview
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.debounce
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.mapLatest
import kotlinx.coroutines.flow.stateIn

/** What `query` found: the search shelf's head of each listing, and each listing's length. */
data class Results(val query: String, val shelf: ShelfSummary?) {
    val isEmpty get() = shelf == null || shelf.artists.isEmpty() && shelf.albums.isEmpty() && shelf.tracks.isEmpty()
}

/**
 * One query across artists, albums and tracks, as `SearchModel.swift`.
 *
 * The results are the search shelf's (`koan_core::shelves`): each section is
 * the head of the listing its "See all" opens. The query is debounced, and a
 * new one cancels the one in flight, so a slow answer never lands over a newer
 * query's. The library moving asks again.
 */
@OptIn(FlowPreview::class, ExperimentalCoroutinesApi::class)
class SearchModel(
    search: suspend (String) -> ShelfSummary,
    library: Flow<Long>,
) : ViewModel() {
    private val _query = MutableStateFlow("")
    val query: StateFlow<String> = _query.asStateFlow()

    /** Survives new queries, so a pick can gather from several; ends with the page. */
    val selection = Selection()

    /** The last query answered. `null` while there is no query. */
    val results: StateFlow<Results?> = combine(
        _query.map { it.trim() }.distinctUntilChanged().debounce { if (it.isEmpty()) 0 else DEBOUNCE_MS },
        library,
    ) { query, _ -> query }
        .mapLatest { query ->
            if (query.isEmpty()) null else Results(query, runCatching { search(query) }.getOrNull())
        }
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    fun setQuery(text: String) {
        _query.value = text
    }

    companion object {
        const val DEBOUNCE_MS = 160L
    }
}
