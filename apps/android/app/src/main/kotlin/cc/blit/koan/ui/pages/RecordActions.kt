package cc.blit.koan.ui.pages

import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import cc.blit.koan.engine.EngineDispatcher
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.nav.Navigator
import cc.blit.koan.state.LibraryModel
import cc.blit.koan.state.PlayerModel
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.koan_ffi.KoanEngineInterface

/** Track ids for an album or an artist, in running order. The engine's `trackIds`, as a seam. */
fun interface TrackIdSource {
    suspend fun trackIds(albumId: Long?, artistId: Long?): List<Long>
}

/** The engine as a [TrackIdSource], asked on the engine dispatcher and never on the thread that called. */
fun KoanEngineInterface.asTrackIdSource(): TrackIdSource = TrackIdSource { album, artist ->
    withContext(EngineDispatcher.dispatcher) { trackIds(album, artist) }
}

/**
 * What the record pages do with the things on them. One object per page, made
 * outside composition and holding no engine: every verb is a model call, or
 * resolves ids through the [TrackIdSource] off the main thread first.
 *
 * Playing something moves to the Queue tab once the engine has taken the new
 * queue ([Navigator.showQueueWhenReady]), as the Swift app does; queueing
 * does not move.
 *
 * Duplicates the shape of the queue and playlist pages' actions on purpose:
 * unify once those land.
 */
class RecordActions(
    private val player: PlayerModel,
    private val library: LibraryModel,
    private val navigator: Navigator,
    private val ids: TrackIdSource,
    private val scope: CoroutineScope,
    /** The queue's version, which moves when the engine has taken a new queue. */
    private val queueVersion: StateFlow<Long>,
) {
    /** Play these tracks, from [startAt]. */
    fun play(trackIds: List<Long>, startAt: Int? = null) {
        if (trackIds.isEmpty()) return
        player.playNow(trackIds, startAt)
        navigator.showQueueWhenReady(queueVersion)
    }

    fun playNext(trackIds: List<Long>) = player.playNext(trackIds)

    fun enqueue(trackIds: List<Long>) = player.enqueue(trackIds)

    fun download(trackIds: List<Long>) = library.downloadToCache(trackIds)

    fun removeDownloads(trackIds: List<Long>) = library.clearDownloads(trackIds)

    fun toggleFavouriteTrack(id: Long) = library.toggleFavouriteTrack(id)

    // MARK: - Whole records and artists

    fun playAlbum(albumId: Long, name: String, shuffled: Boolean) = playResolved(name, shuffled) {
        ids.trackIds(albumId, null)
    }

    fun playArtist(artistId: Long, name: String, shuffled: Boolean) = playResolved(name, shuffled) {
        ids.trackIds(null, artistId)
    }

    fun playNextArtist(artistId: Long) = withIds { player.playNext(ids.trackIds(null, artistId)) }

    fun enqueueArtist(artistId: Long) = withIds { player.enqueue(ids.trackIds(null, artistId)) }

    // MARK: - A pick of albums, in the order they were ticked

    fun playAlbums(albumIds: List<Long>, shuffled: Boolean = false) {
        if (albumIds.isEmpty()) return
        playResolved(countOf(albumIds.size, "album"), shuffled) { tracksOf(albumIds) }
    }

    fun playNextAlbums(albumIds: List<Long>) = withIds { player.playNext(tracksOf(albumIds)) }

    fun enqueueAlbums(albumIds: List<Long>) = withIds { player.enqueue(tracksOf(albumIds)) }

    fun downloadAlbums(albumIds: List<Long>) = withIds { library.downloadToCache(tracksOf(albumIds)) }

    private suspend fun tracksOf(albumIds: List<Long>): List<Long> = albumIds.flatMap { ids.trackIds(it, null) }

    private fun playResolved(name: String, shuffled: Boolean, resolve: suspend () -> List<Long>) {
        player.playNow(name, shuffled, resolve)
        navigator.showQueueWhenReady(queueVersion)
    }

    private fun withIds(body: suspend () -> Unit) {
        scope.launch { body() }
    }
}

/**
 * The page's actions, made once per runtime. The queue's version is turned into
 * a state flow here (the navigator wants one to wait on) on the page's scope;
 * the engine is handed on as a [TrackIdSource] and not called from composition.
 */
@Composable
fun rememberRecordActions(runtime: KoanRuntime): RecordActions {
    val scope = rememberCoroutineScope()
    return remember(runtime) {
        val queue = runtime.mirror.queue
        val version = queue.map { it.version }.stateIn(scope, SharingStarted.Eagerly, queue.value.version)
        RecordActions(runtime.player, runtime.library, runtime.navigator, runtime.engine.asTrackIdSource(), scope, version)
    }
}
