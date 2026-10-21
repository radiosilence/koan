@file:OptIn(ExperimentalCoroutinesApi::class)

package cc.blit.koan.ui.playlists

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import cc.blit.koan.engine.EngineMirror
import cc.blit.koan.ffi.Playlist
import cc.blit.koan.ffi.PlaylistEntry
import cc.blit.koan.ffi.RunRole
import kotlinx.coroutines.ExperimentalCoroutinesApi
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

    /** Where the playlists sit in the list. This device's alone: a server has nowhere to keep it. */
    fun arrange(playlistIds: List<Long>) = mirror.launch { reorderPlaylists(playlistIds) }

    fun reorder(playlistId: Long, entryIds: List<Long>, failed: () -> Unit) = mirror.launch {
        try {
            reorderPlaylist(playlistId, entryIds)
        } catch (e: Exception) {
            failed()
            throw e
        }
    }

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

    data class Heading(val album: String, val albumId: Long?, val entries: List<PlaylistEntry>) : PlaylistRow {
        override val key get() = "album:${entries.first().entryId}"
    }

    data class Entry(val entry: PlaylistEntry) : PlaylistRow {
        override val key get() = "entry:${entry.entryId}"
    }
}

/**
 * The rows for `entries`, grouped as the engine grouped them or flat: a heading
 * above each entry that opens a record's run, the run beneath it.
 */
fun playlistRows(entries: List<PlaylistEntry>, grouped: Boolean): List<PlaylistRow> {
    if (!grouped) return entries.map(PlaylistRow::Entry)
    val rows = ArrayList<PlaylistRow>(entries.size + entries.size / 8)
    var index = 0
    while (index < entries.size) {
        val first = entries[index]
        if (first.run != RunRole.OPENS) {
            rows += PlaylistRow.Entry(first)
            index++
            continue
        }
        var end = index + 1
        while (end < entries.size && entries[end].run == RunRole.CONTINUES) end++
        val run = entries.subList(index, end).toList()
        rows += PlaylistRow.Heading(first.track.albumTitle, first.track.albumId, run)
        run.mapTo(rows, PlaylistRow::Entry)
        index = end
    }
    return rows
}

/** A row grouping leaves without a heading above it: on no record, or a single. */
val PlaylistEntry.standsAlone get() = run == RunRole.ALONE || run == RunRole.SINGLE
