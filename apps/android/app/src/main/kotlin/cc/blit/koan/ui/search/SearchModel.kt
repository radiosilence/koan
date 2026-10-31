package cc.blit.koan.ui.search

import cc.blit.koan.ui.Read
import cc.blit.koan.ui.library.Selection
import cc.blit.koan.ui.library.playableSelection
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import cc.blit.koan.ffi.ShelfSummary
import cc.blit.koan.ui.read
import cc.blit.koan.ui.value
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
import kotlinx.coroutines.flow.update

/** What `query` found: the search shelf's head of each listing, and each listing's length; or why the search failed. */
data class Results(val query: String, val read: Read<ShelfSummary>) {
    val shelf get() = read.value
    val isEmpty get() = shelf?.run { artists.isEmpty() && albums.isEmpty() && tracks.isEmpty() } ?: true
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
    val selection = playableSelection()

    private val retries = MutableStateFlow(0)

    /** The last query answered. `null` while there is no query. */
    val results: StateFlow<Results?> = combine(
        _query.map { it.trim() }.distinctUntilChanged().debounce { if (it.isEmpty()) 0 else DEBOUNCE_MS },
        library,
        retries,
    ) { query, _, _ -> query }
        .mapLatest { query ->
            if (query.isEmpty()) null else Results(query, read { search(query) })
        }
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    fun setQuery(text: String) {
        _query.value = text
    }

    /** Ask again after a search that failed. */
    fun retry() = retries.update { it + 1 }

    companion object {
        const val DEBOUNCE_MS = 160L
    }
}
