package cc.blit.koan.ui.queue

import androidx.compose.runtime.Immutable
import uniffi.koan_ffi.QueueItem

/**
 * A record in the queue: a contiguous run of items off one album. Not a
 * library album: it stands for queue items, so its actions are the queue's.
 */
@Immutable
data class QueueGroup(
    val id: String,
    val albumArtist: String,
    val album: String,
    val items: List<QueueItem>,
) {
    val year: String? get() = items.firstOrNull()?.year

    val title: String get() = album.ifEmpty { albumArtist.ifEmpty { "Unknown Artist" } }
}

/**
 * One row of the list. Album headings are rows in their own right, not
 * decoration on the first track: that is what lets an album be selected as a
 * unit, and stops selecting a track from lighting the heading above it.
 */
sealed interface QueueLine {
    /** The list key: a track's is its queue item id; a heading's is synthetic. */
    val id: String

    /** The queue items this row stands for. */
    val itemIds: List<String>

    /** The `LazyColumn` content type, so rows of one shape reuse each other. */
    val contentType: String

    @Immutable
    data class Heading(val group: QueueGroup) : QueueLine {
        override val id: String get() = "album:${group.id}"
        override val itemIds: List<String> get() = group.items.map { it.queueItemId }
        override val contentType: String get() = "heading"
    }

    /**
     * One track, as it was when the layout was built. Only its identity is
     * trusted: the row draws the item as the queue reads *now*.
     */
    @Immutable
    data class Track(val item: QueueItem, val showArtist: Boolean, val artwork: Boolean) : QueueLine {
        override val id: String get() = item.queueItemId
        override val itemIds: List<String> get() = listOf(item.queueItemId)
        override val contentType: String get() = if (artwork) "trackArt" else "track"
    }
}

/** Where a queue edit goes: these items, before or after `target`. */
data class MoveOp(val itemIds: List<String>, val target: String, val after: Boolean)

object QueueLayout {
    /**
     * The rows of a queue. Grouped, by *contiguous runs* rather than sorting:
     * queue order is the user's, and collapsing two separate visits to the
     * same record into one heading would misrepresent it. A track with no
     * album stands alone, and a record that is only one track whose title is
     * the album's folds into a single row with its own sleeve. Ungrouped,
     * every row is a track with a sleeve and says what it is itself.
     */
    fun build(items: List<QueueItem>, grouped: Boolean): List<QueueLine> {
        if (!grouped) return items.map { QueueLine.Track(it, showArtist = true, artwork = true) }
        val rows = ArrayList<QueueLine>(items.size + items.size / 8)
        var index = 0
        while (index < items.size) {
            val first = items[index]
            if (first.album.isEmpty()) {
                rows += QueueLine.Track(first, showArtist = first.artist != first.albumArtist, artwork = false)
                index++
                continue
            }
            var end = index + 1
            while (end < items.size && items[end].album == first.album && items[end].albumArtist == first.albumArtist) end++
            val run = items.subList(index, end)
            if (run.size == 1 && first.title.trim().equals(first.album.trim(), ignoreCase = true)) {
                rows += QueueLine.Track(first, showArtist = true, artwork = true)
            } else {
                rows += QueueLine.Heading(QueueGroup(first.queueItemId, first.albumArtist, first.album, run.toList()))
                for (item in run) {
                    rows += QueueLine.Track(item, showArtist = item.artist != item.albumArtist, artwork = false)
                }
            }
            index = end
        }
        return rows
    }

    /** The queue items behind a set of row ids: a heading stands for its whole run. */
    fun itemIds(rowIds: Set<String>, lines: List<QueueLine>): List<String> =
        lines.filter { it.id in rowIds }.flatMap { it.itemIds }

    /** Where a queue item's row is, or -1. */
    fun indexOfItem(lines: List<QueueLine>, itemId: String?): Int =
        if (itemId == null) -1 else lines.indexOfFirst { it is QueueLine.Track && it.item.queueItemId == itemId }

    /**
     * The queue edit for a row dropped somewhere. `lines` are the rows in the
     * order they were left in, `draggedId` the row that was moved, and
     * `selected` the queue items selected: dragging one of several selected
     * rows moves them all, keeping their order, as it does on the Mac.
     *
     * Anchors to the row the drop lands *before*, not to the row above it
     * with an insert-after: "after" has no way to say "at the very top". The
     * first row below the drop that is not itself moving is the target; with
     * none, the move is to the end, after the last that stays.
     *
     * Null when nothing changes: the rows dropped where they began.
     */
    fun moveOp(
        lines: List<QueueLine>,
        draggedId: String,
        selected: Set<String>,
        queueOrder: List<String>,
    ): MoveOp? {
        val dragged = lines.firstOrNull { it.id == draggedId } ?: return null
        val moving: List<String> =
            if (dragged is QueueLine.Track && dragged.item.queueItemId in selected && selected.size > 1) {
                queueOrder.filter { it in selected }
            } else {
                dragged.itemIds
            }
        if (moving.isEmpty()) return null
        val movingSet = moving.toSet()
        val at = lines.indexOfFirst { it.id == draggedId }
        val below = lines.drop(at + 1).firstOrNull { line -> line.itemIds.none { it in movingSet } }
        val op = if (below != null) {
            MoveOp(moving, below.itemIds.first(), after = false)
        } else {
            val last = lines.lastOrNull { line -> line.itemIds.none { it in movingSet } } ?: return null
            MoveOp(moving, last.itemIds.last(), after = true)
        }
        return if (resulting(queueOrder, op) == queueOrder) null else op
    }

    /** The queue order an edit produces: what the engine will hold after it. */
    fun resulting(order: List<String>, op: MoveOp): List<String> {
        val moving = op.itemIds.toSet()
        val rest = order.filter { it !in moving }
        val at = rest.indexOf(op.target)
        if (at < 0) return order
        return rest.take(if (op.after) at + 1 else at) + op.itemIds + rest.drop(if (op.after) at + 1 else at)
    }
}

/**
 * What is picked out of the queue, by row id. Immutable; the page holds the
 * current one. A heading's id is in the set when the whole album was picked.
 */
@Immutable
data class QueueSelection(val rows: Set<String> = emptySet(), val active: Boolean = false) {
    val isEmpty: Boolean get() = rows.isEmpty()

    fun toggle(id: String): QueueSelection =
        copy(rows = if (id in rows) rows - id else rows + id, active = true)

    fun start(id: String): QueueSelection = copy(rows = rows + id, active = true)

    fun all(lines: List<QueueLine>): QueueSelection = copy(rows = lines.mapTo(HashSet()) { it.id }, active = true)

    /** Out of selecting altogether. */
    fun clear(): QueueSelection = QueueSelection()

    /** Rows that are no longer in the list are no longer picked. */
    fun retain(lines: List<QueueLine>): QueueSelection {
        if (rows.isEmpty()) return this
        val live = lines.mapTo(HashSet()) { it.id }
        val kept = rows.filterTo(HashSet()) { it in live }
        return if (kept.size == rows.size) this else copy(rows = kept)
    }

    /** The queue items picked, in queue order, each once (a track picked inside a picked album counts once). */
    fun itemIds(lines: List<QueueLine>): List<String> =
        QueueLayout.itemIds(rows, lines).distinct()
}
