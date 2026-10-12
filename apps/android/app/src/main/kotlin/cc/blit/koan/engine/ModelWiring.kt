package cc.blit.koan.engine

import cc.blit.koan.art.ArtworkSource
import cc.blit.koan.art.ArtworkStore
import cc.blit.koan.state.ActivityModel
import cc.blit.koan.state.EngineMirror
import cc.blit.koan.state.LibraryModel
import cc.blit.koan.state.PlaylistsModel
import cc.blit.koan.state.Resource
import cc.blit.koan.theme.KoanAccent
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.mapLatest
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch

/**
 * What the models do when the mirror says something moved, in one place.
 *
 * The mirror publishes slices; a model that reads the library does not poll
 * (Swift `AppState` wires the same). A library change reloads the section on
 * screen, favourites and stats, the playlists and the artwork that was absent;
 * a play recorded reloads what history derives; the engine's own scan and sync
 * show as rows of the activity registry; the sync's progress lands on every
 * sync row; and the colour the record playing gives becomes the accent.
 *
 * Launched on [scope] (the engine's dispatcher), so every model is touched
 * from the one thread it is confined to.
 */
class ModelWiring(
    private val scope: CoroutineScope,
    private val mirror: EngineMirror,
    private val library: LibraryModel,
    private val playlists: PlaylistsModel,
    private val activity: ActivityModel,
    private val artwork: ArtworkStore,
) {
    private val _accent = MutableStateFlow(KoanAccent.Mint)

    /** The accent of the record playing: mint with nothing playing or no colour in the sleeve. */
    val accent: StateFlow<KoanAccent> = _accent.asStateFlow()

    @OptIn(ExperimentalCoroutinesApi::class)
    fun start() {
        // The first value is the initial load: the library is read once at
        // start, and again whenever its rows move.
        scope.launch {
            mirror.libraryVersion.collect {
                artwork.libraryChanged()
                library.libraryChanged()
                playlists.load()
            }
        }
        scope.launch { mirror.historyVersion.collect { library.historyChanged() } }
        scope.launch { mirror.syncProgress.collect { activity.showSync(it) } }
        scope.launch {
            mirror.tasks.collect { tasks ->
                activity.mirror("Scanning library", running = tasks.scanning, uses = Resource.LocalLibrary, cancellable = true)
                activity.mirror(
                    "Syncing with server", running = tasks.syncing,
                    uses = setOf(Resource.RemoteTracks), followsSync = true,
                )
            }
        }
        scope.launch {
            mirror.playback
                .map { now -> now.entry?.let { it.albumId?.let(ArtworkSource::Album) ?: it.trackId?.let(ArtworkSource::Track) } }
                .distinctUntilChanged()
                .mapLatest { source -> source?.let { artwork.accent(it) } ?: KoanAccent.Mint }
                .collect { _accent.value = it }
        }
    }
}
