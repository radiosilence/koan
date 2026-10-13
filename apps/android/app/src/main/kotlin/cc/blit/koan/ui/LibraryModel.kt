package cc.blit.koan.ui

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import cc.blit.koan.KoanApp
import cc.blit.koan.ffi.Album
import cc.blit.koan.ffi.AlbumPage
import cc.blit.koan.ffi.AlbumSort
import cc.blit.koan.ffi.BrowseFilter
import cc.blit.koan.ffi.KoanException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

data class LibraryState(
    /** `null` until the engine has said whether a server is signed in. */
    val signedIn: Boolean? = null,
    val busy: String? = null,
    val error: String? = null,
    val albums: List<Album> = emptyList(),
    val album: AlbumPage? = null,
)

class LibraryModel(app: Application) : AndroidViewModel(app) {
    private val engine = (app as KoanApp).engine
    private val _state = MutableStateFlow(LibraryState())
    val state: StateFlow<LibraryState> = _state.asStateFlow()

    val nowPlaying = engine.nowPlaying
    val playhead = engine.playhead

    init {
        viewModelScope.launch {
            val signedIn = engine.get().settings().remoteSignedIn
            _state.update { it.copy(signedIn = signedIn) }
            if (signedIn) loadAlbums()
        }
    }

    fun signIn(url: String, username: String, password: String) = work("Signing in") {
        engine.get().signInRemote(url.trim(), username.trim(), password)
        _state.update { it.copy(signedIn = true, busy = "Syncing the library") }
        engine.get().syncRemote()
        loadAlbums()
    }

    fun openAlbum(id: Long?) {
        if (id == null) {
            _state.update { it.copy(album = null) }
            return
        }
        work(null) { _state.update { s -> s.copy(album = engine.get().albumPage(id)) } }
    }

    fun play(trackIds: List<Long>, index: Int) = engine.launch {
        replaceQueue(trackIds, index.toUInt(), false)
    }

    fun togglePlayPause() = engine.launch { togglePlayPause() }

    fun next() = engine.launch { next() }

    private suspend fun loadAlbums() {
        val albums = engine.get().albums(null, AlbumSort.ARTIST, 0, null, NO_FILTER)
        _state.update { it.copy(albums = albums) }
    }

    private fun work(label: String?, block: suspend () -> Unit) {
        viewModelScope.launch {
            _state.update { it.copy(busy = label, error = null) }
            try {
                block()
            } catch (e: KoanException) {
                _state.update { it.copy(error = e.describe()) }
            } finally {
                _state.update { it.copy(busy = null) }
            }
        }
    }

    private fun KoanException.describe() = when (this) {
        is KoanException.Database -> detail
        is KoanException.Audio -> detail
        is KoanException.Player -> detail
        is KoanException.NotFound -> detail
        is KoanException.BadArgument -> detail
        is KoanException.Remote -> detail
        is KoanException.NeedsSampleRate -> detail
    }

    private companion object {
        val NO_FILTER = BrowseFilter(
            favourites = false,
            recent = false,
            downloaded = false,
            lossless = false,
            codec = null,
            yearFrom = null,
            yearTo = null,
            genre = null,
        )
    }
}
