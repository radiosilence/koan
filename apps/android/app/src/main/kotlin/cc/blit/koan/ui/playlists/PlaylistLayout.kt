package cc.blit.koan.ui.playlists

import uniffi.koan_ffi.Playlist
import uniffi.koan_ffi.PlaylistEntry
import uniffi.koan_ffi.Track

/**
 * A row of a playlist page: an album heading over a run, or one entry. The
 * port of `PlaylistView.Row`.
 *
 * An entry carries its position in the playlist rather than finding itself
 * by id each time it is asked, and its identity is the *entry* id: a playlist
 * may hold one track twice, and selecting one copy must not light the other.
 */
sealed interface PlaylistLine {
    /** Stable across rebuilds for as long as the entry (or the run's first entry) is. */
    val id: String
    val contentType: String

    class Heading(
        override val id: String,
        val album: String,
        val artist: String,
        val albumId: Long?,
        /** A track of the run, for a sleeve where the library does not know the record. */
        val trackId: Long,
        val positions: List<Int>,
    ) : PlaylistLine {
        override val contentType: String get() = "heading"
    }

    class Entry(val entry: PlaylistEntry, val position: Int) : PlaylistLine {
        override val id: String get() = entryKey(entry.entryId)
        override val contentType: String get() = "entry"
    }
}

fun entryKey(entryId: Long): String = "entry:$entryId"

/** Where an entry is dropped when it is moved, in the terms `PlaylistsModel.reorderPlaylists` takes. */
data class PlaylistMove(val moving: List<Long>, val onto: Long)

/**
 * Everything the playlist pages decide that is not drawing: how entries are
 * grouped, what a selection stands for, and what a drag means to the model.
 * Kept free of Compose and of the engine so it is checked on the JVM.
 */
object PlaylistLayout {
    /**
     * The rows. Flat is one per entry. Grouped puts a heading over each
     * *contiguous run* of one record: playlist order is the person's, so two
     * separate visits to a record are two headings rather than one. An entry
     * with no album takes no heading.
     */
    fun build(entries: List<PlaylistEntry>, grouped: Boolean): List<PlaylistLine> {
        if (!grouped) return entries.mapIndexed { at, entry -> PlaylistLine.Entry(entry, at) }
        val lines = ArrayList<PlaylistLine>(entries.size + entries.size / 4)
        var index = 0
        while (index < entries.size) {
            val first = entries[index]
            if (first.track.albumTitle.isEmpty()) {
                lines += PlaylistLine.Entry(first, index)
                index += 1
                continue
            }
            var end = index + 1
            while (end < entries.size && sameRecord(entries[end].track, first.track)) end += 1
            lines += PlaylistLine.Heading(
                id = "album:${first.entryId}",
                album = first.track.albumTitle,
                artist = first.track.albumArtistName,
                albumId = first.track.albumId,
                trackId = first.track.id,
                positions = (index until end).toList(),
            )
            for (at in index until end) lines += PlaylistLine.Entry(entries[at], at)
            index = end
        }
        return lines
    }

    /** Two records can share a title; the id tells them apart where there is one. */
    private fun sameRecord(a: Track, b: Track): Boolean {
        val x = a.albumId
        val y = b.albumId
        if (x != null && y != null) return x == y
        return a.albumTitle == b.albumTitle && a.albumArtistName == b.albumArtistName
    }

    /** The playlist positions a set of row ids stands for, in playlist order. A heading stands for its whole run. */
    fun positions(lines: List<PlaylistLine>, ids: Set<String>): List<Int> {
        if (ids.isEmpty()) return emptyList()
        val found = sortedSetOf<Int>()
        for (line in lines) {
            if (line.id !in ids) continue
            when (line) {
                is PlaylistLine.Heading -> found.addAll(line.positions)
                is PlaylistLine.Entry -> found.add(line.position)
            }
        }
        return found.toList()
    }

    fun entryIds(entries: List<PlaylistEntry>, positions: List<Int>): List<Long> =
        positions.mapNotNull { entries.getOrNull(it)?.entryId }

    fun trackIds(entries: List<PlaylistEntry>, positions: List<Int>): List<Long> =
        positions.mapNotNull { entries.getOrNull(it)?.track?.id }

    /** The entries in the order the rows show them, headings aside. */
    fun shownOrder(lines: List<PlaylistLine>): List<Long> =
        lines.mapNotNull { (it as? PlaylistLine.Entry)?.entry?.entryId }

    /**
     * The full order to send after a drag, or null when the drop left things
     * where they were. [shown] is the rows as the finger left them. When the
     * dragged entry is one of several picked, the picked entries travel
     * together, in their old relative order, to where the dragged one landed;
     * otherwise only the dragged entry has moved.
     */
    fun reordered(entries: List<PlaylistEntry>, shown: List<PlaylistLine>, dragged: Long, picked: Set<Long>): List<Long>? {
        val original = entries.map { it.entryId }
        val order = shownOrder(shown)
        if (order.size != original.size) return null
        val result = if (dragged in picked && picked.size > 1) {
            val block = original.filter { it in picked }
            val at = order.indexOf(dragged)
            if (at < 0) return null
            val rest = order.filterNot { it in picked }
            val insertAt = order.subList(0, at).count { it !in picked }
            rest.toMutableList().apply { addAll(insertAt, block) }
        } else {
            order
        }
        return result.takeIf { it != original }
    }

    /**
     * Playlists are reordered by "put these where that one sits", which lands
     * them *before* the target. Moving a row up is that directly. Moving it
     * down by naming the row it now sits under would land it one short, and
     * the end of the list has no row to name; so a move down is expressed the
     * other way round: the rows it passed go before it. Same result, and every
     * place in the list can be asked for.
     */
    fun movePlaylist(order: List<Long>, from: Int, to: Int): PlaylistMove? {
        if (from == to || from !in order.indices || to !in order.indices) return null
        return if (to < from) {
            PlaylistMove(listOf(order[from]), onto = order[to])
        } else {
            PlaylistMove(order.subList(from + 1, to + 1).toList(), onto = order[from])
        }
    }

    /** Whether the playlist takes edits to what it holds, as `PlaylistsModel.fillable` decides. */
    fun fillable(playlist: Playlist?): Boolean = playlist?.readonly != true

    /** Its file names a playlist read from one, so it cannot be renamed here, as `PlaylistsModel.rename` decides. */
    fun renamable(playlist: Playlist?): Boolean = playlist != null && !playlist.fromFile

    /** "1 track", "12 tracks". */
    fun count(n: Int, noun: String = "track"): String = if (n == 1) "1 $noun" else "$n ${noun}s"

    /** The header's line under the name: how many, how long, and whether a server keeps it too. */
    fun summary(entries: List<PlaylistEntry>, synced: Boolean): String = buildList {
        add(count(entries.size))
        val total = entries.sumOf { it.track.durationMs ?: 0L }
        if (total > 0) add(duration(total))
        if (synced) add("synced")
    }.joinToString(" · ")

    /** 3:05, or 1:02:09. */
    fun duration(ms: Long): String {
        val total = (ms / 1000).toInt()
        val (h, m, s) = Triple(total / 3600, total % 3600 / 60, total % 60)
        return if (h > 0) "%d:%02d:%02d".format(h, m, s) else "%d:%02d".format(m, s)
    }

    /** A name worth creating a playlist with. */
    fun nameable(raw: String): String? = raw.trim().takeIf { it.isNotEmpty() }

    /** The text a delete asks about. */
    fun deleteQuestion(playlist: Playlist?): String =
        "Delete ${playlist?.name?.let { "\"$it\"" } ?: "this playlist"}? The tracks stay in the library."
}
