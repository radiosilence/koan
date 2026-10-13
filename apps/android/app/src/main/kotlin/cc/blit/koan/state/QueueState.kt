package cc.blit.koan.state

import androidx.compose.runtime.Immutable
import uniffi.koan_ffi.EntryStatus
import uniffi.koan_ffi.QueueItem

/**
 * The queue's rows as last sent whole, each with its status as it was then.
 *
 * Moves when the queue is edited, and not when a track changes or a download
 * lands: those arrive as patches to [QueueState]. A long list lays itself out
 * from this and has each row it draws read [QueueState.item], so a track
 * change redraws the rows on screen rather than regrouping every row.
 *
 * Compared by identity, on purpose. A snapshot is only ever published because
 * the engine said the queue differs, and comparing five thousand rows to
 * confirm it would be the one O(n) step on the path.
 */
@Immutable
class QueueRows(val items: List<QueueItem>, val version: Long) {
    companion object {
        val Empty = QueueRows(emptyList(), 0)
    }
}

/**
 * The queue as it reads now: [QueueRows] with the latest patch applied, and
 * where each row is. Identity-compared, as [QueueRows].
 */
@Immutable
class QueueState internal constructor(
    val items: List<QueueItem>,
    val version: Long,
    private val rowOf: Map<String, Int>,
) {
    /** One row, by queue item id. */
    fun item(id: String): QueueItem? = rowOf[id]?.let { items[it] }

    /** Where a queue item sits, or -1. */
    fun indexOf(id: String): Int = rowOf[id] ?: -1

    companion object {
        val Empty = QueueState(emptyList(), 0, emptyMap())
    }
}

/**
 * Queue entries indexed by library track id and by the playlist row they came
 * from, so a library row can say what the queue knows about it without
 * scanning the queue once per row.
 *
 * Keyed on the playlist entry as well as the track, because a playlist may
 * hold one track twice and a by-track index can answer for only one of them.
 * Both are derived from one queue slice and move with it: two indexes patched
 * in step can disagree, derived from a single slice they cannot.
 */
@Immutable
class QueueIndex internal constructor(
    val byTrack: Map<Long, QueueItem>,
    val byPlaylistEntry: Map<Long, QueueItem>,
) {
    companion object {
        val Empty = QueueIndex(emptyMap(), emptyMap())
    }
}

/**
 * Of the rows holding one track: a track queued twice prefers the entry that
 * is doing something over one still sitting idle.
 */
internal fun preferred(rows: List<QueueItem>): QueueItem? =
    rows.fold<QueueItem, QueueItem?>(null) { held, next ->
        when {
            held == null -> next
            next.status == EntryStatus.QUEUED -> held
            else -> next
        }
    }
