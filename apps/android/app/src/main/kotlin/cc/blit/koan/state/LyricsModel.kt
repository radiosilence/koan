package cc.blit.koan.state

import androidx.compose.runtime.Immutable
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.withContext
import uniffi.koan_ffi.KoanEngineInterface
import uniffi.koan_ffi.LyricLine
import uniffi.koan_ffi.Lyrics

/**
 * What was found, and for which track. Held together so the words can only be
 * shown beside the track they belong to: a load that loses a race with a track
 * change leaves nothing on screen, never another song.
 */
@Immutable
data class LyricsState(val trackId: Long?, val lyrics: Lyrics?, val loading: Boolean) {
    companion object {
        val None = LyricsState(null, null, false)
    }
}

/**
 * Lyrics for the track playing, from the engine's cache and then LRCLIB. The
 * port of the loading half of `LyricsPanel.swift`; the engine hands back
 * already-parsed LRC lines, so this only has to fetch them and a view only has
 * to pick the current one ([LyricsTimeline]).
 *
 * The engine is called on [dispatcher], never on the caller's thread: a
 * composable runs [follow] from a `LaunchedEffect` on Main, and the crossing
 * is not Main's to make.
 */
class LyricsModel(
    private val engine: KoanEngineInterface,
    private val mirror: EngineMirror,
    private val dispatcher: CoroutineDispatcher,
) {
    private val _state = MutableStateFlow(LyricsState.None)
    val state: StateFlow<LyricsState> = _state.asStateFlow()

    /**
     * Follow the track playing until cancelled. Cache first so the panel
     * fills at once, then LRCLIB for a miss or a plain copy (the engine
     * decides whether a plain copy is old enough to ask again). Run it only
     * while the words are on screen: it is the network call.
     */
    @OptIn(ExperimentalCoroutinesApi::class)
    suspend fun follow() {
        mirror.playback.map { it.entry?.trackId }.distinctUntilChanged().collectLatest { trackId ->
            if (trackId == null) {
                _state.value = LyricsState.None
                return@collectLatest
            }
            if (_state.value.trackId == trackId && _state.value.lyrics?.synced == true) return@collectLatest
            _state.value = LyricsState(trackId, null, loading = false)
            val cached = ask { engine.lyrics(trackId) }
            _state.value = LyricsState(trackId, cached, loading = cached?.synced != true)
            if (cached?.synced == true) return@collectLatest
            val fetched = ask { engine.fetchLyrics(trackId) }
            _state.value = LyricsState(trackId, fetched ?: cached, loading = false)
        }
    }

    private suspend fun ask(call: suspend () -> Lyrics?): Lyrics? = withContext(dispatcher) {
        try {
            call()
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            null
        }
    }
}

/** Which line is being sung, and when it next changes. */
object LyricsTimeline {
    /** The last line whose timestamp has passed at `positionSecs`, or -1 before the first. */
    fun currentIndex(lines: List<LyricLine>, positionSecs: Double): Int =
        lines.indexOfLast { it.timeSecs <= positionSecs }

    /**
     * How long until the line changes, from `positionSecs`: the next
     * timestamp's distance, or null when there is no next line. A lyric line
     * changes when a timestamp passes and every timestamp is known, so a view
     * sleeps until the next one instead of asking where the playhead is: one
     * wake per line, none while paused.
     */
    fun millisToNext(lines: List<LyricLine>, positionSecs: Double): Long? =
        lines.firstOrNull { it.timeSecs > positionSecs }?.let { ((it.timeSecs - positionSecs) * 1000).toLong() + 1 }
}
