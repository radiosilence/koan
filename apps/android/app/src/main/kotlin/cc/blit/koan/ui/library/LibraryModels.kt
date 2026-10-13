package cc.blit.koan.ui.library

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import cc.blit.koan.engine.EngineMirror
import cc.blit.koan.ffi.Album
import cc.blit.koan.ffi.AlbumPage
import cc.blit.koan.ffi.AlbumSort
import cc.blit.koan.ffi.BrowseFilter
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.mapLatest
import kotlinx.coroutines.flow.stateIn

/** Every filter off: the whole library. */
val NoFilter = BrowseFilter(
    favourites = false,
    recent = false,
    downloaded = false,
    lossless = false,
    codec = null,
    yearFrom = null,
    yearTo = null,
    genre = null,
)

/** The albums, asked for again whenever the library moves. */
class LibraryModel(mirror: EngineMirror) : ViewModel() {
    val albums: StateFlow<List<Album>> = mirror.library
        .mapLatest { runCatching { mirror.engine().albums(null, AlbumSort.ARTIST, 0, null, NoFilter) }.getOrDefault(emptyList()) }
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), emptyList())
}

class AlbumModel(mirror: EngineMirror, albumId: Long) : ViewModel() {
    val page: StateFlow<AlbumPage?> = mirror.library
        .mapLatest { runCatching { mirror.engine().albumPage(albumId) }.getOrNull() }
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)
}
