package cc.blit.koan.ui.playlists

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import cc.blit.koan.engine.EngineMirror
import cc.blit.koan.ffi.Playlist
import cc.blit.koan.ffi.PlaylistEntry
import cc.blit.koan.ffi.Track
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.mapLatest
import kotlinx.coroutines.flow.stateIn

/**
 * The playlists, asked for again whenever the library moves. Writing a
 * playlist is a library change like any other, so every edit, here or synced
 * from the server, reaches the list the same way.
 */
class PlaylistsModel(mirror: EngineMirror) : ViewModel() {
    val playlists: StateFlow<List<Playlist>?> = mirror.library
        .mapLatest { runCatching { mirror.engine().playlists() }.getOrDefault(emptyList()) }
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)
}

/** One playlist: its row, its entries and the records its mosaic is made of, read together. */
data class PlaylistContents(val playlist: Playlist?, val entries: List<PlaylistEntry>, val covers: List<Long>)

class PlaylistModel(mirror: EngineMirror, playlistId: Long) : ViewModel() {
    val contents: StateFlow<PlaylistContents?> = mirror.library
        .mapLatest {
            runCatching {
                val engine = mirror.engine()
                PlaylistContents(
                    engine.playlists().firstOrNull { it.id == playlistId },
                    engine.playlistTracks(playlistId),
                    engine.playlistCoverAlbumIds(playlistId),
                )
            }.getOrNull()
        }
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)
}

/**
 * What can be done to playlists. Each call returns at once; the engine writes,
 * pushes to the server in the background, and moves the library version,
 * which is what brings the pages up to date.
 */
class PlaylistEdits(private val mirror: EngineMirror) {
    suspend fun create(name: String, trackIds: List<Long> = emptyList()): Playlist? =
        name.trim().takeIf { it.isNotEmpty() }?.let { trimmed ->
            runCatching { mirror.engine().createPlaylist(trimmed, trackIds) }.getOrNull()
        }

    fun rename(playlist: Playlist, name: String) {
        val trimmed = name.trim()
        // A playlist file names its playlist.
        if (playlist.fromFile || trimmed.isEmpty()) return
        mirror.launch { renamePlaylist(playlist.id, trimmed) }
    }

    fun delete(playlistId: Long) = mirror.launch { deletePlaylist(playlistId) }

    fun reorder(playlistId: Long, entryIds: List<Long>) = mirror.launch { reorderPlaylist(playlistId, entryIds) }

    fun remove(playlistId: Long, entryIds: List<Long>) {
        if (entryIds.isNotEmpty()) mirror.launch { removeFromPlaylist(playlistId, entryIds) }
    }

    /** Shuffle the playlist itself, for good; playing it shuffled leaves it alone. */
    fun shuffleOrder(playlistId: Long) = mirror.launch { shufflePlaylist(playlistId) }

    fun setGrouped(playlistId: Long, grouped: Boolean) = mirror.launch { setPlaylistGrouped(playlistId, grouped) }

    /** Replace the queue with the playlist and play it from `startEntry`, or the top. */
    fun play(playlistId: Long, startEntry: Long? = null, shuffled: Boolean = false) =
        mirror.launch { playPlaylist(playlistId, startEntry, shuffled) }
}

/** Whether the playlist takes adds, removals and reorders: rules or a file decide a read-only one's contents. */
val Playlist.fillable get() = !readonly

/** A playlist page's rows: a record's heading over a run of its tracks, or one entry. */
sealed interface PlaylistRow {
    val key: String

    data class Heading(val album: String, val artist: String, val albumId: Long?, val entries: List<PlaylistEntry>) : PlaylistRow {
        override val key get() = "album:${entries.first().entryId}"
    }

    data class Entry(val entry: PlaylistEntry) : PlaylistRow {
        override val key get() = "entry:${entry.entryId}"
    }
}

/**
 * The rows for `entries`, grouped or flat. Grouped, each contiguous run of one
 * record gets a heading: playlist order is the person's, so two separate visits
 * to a record are two headings rather than one.
 */
fun playlistRows(entries: List<PlaylistEntry>, grouped: Boolean): List<PlaylistRow> {
    if (!grouped) return entries.map(PlaylistRow::Entry)
    return runs(entries) { a, b -> a.track.albumTitle.isNotEmpty() && sameRecord(a.track, b.track) }.flatMap { run ->
        val first = run.first().track
        val heading = if (first.albumTitle.isEmpty()) {
            emptyList()
        } else {
            listOf(PlaylistRow.Heading(first.albumTitle, first.albumArtistName, first.albumId, run))
        }
        heading + run.map(PlaylistRow::Entry)
    }
}

/** Two records can share a title; the id tells them apart where there is one. */
private fun sameRecord(a: Track, b: Track): Boolean {
    val ids = a.albumId to b.albumId
    if (ids.first != null && ids.second != null) return ids.first == ids.second
    return a.albumTitle == b.albumTitle && a.albumArtistName == b.albumArtistName
}

/** `items` cut into maximal runs whose neighbours are `same` as the run's first. */
fun <T> runs(items: List<T>, same: (T, T) -> Boolean): List<List<T>> {
    val out = mutableListOf<MutableList<T>>()
    for (item in items) {
        val run = out.lastOrNull()
        if (run != null && same(run.first(), item)) run.add(item) else out.add(mutableListOf(item))
    }
    return out
}
