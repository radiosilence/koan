package cc.blit.koan.ui.queue

import cc.blit.koan.ffi.QueueItem

/**
 * A row of the queue list: a record's heading, one of its tracks, or a track
 * that is its whole record, folded into one row.
 *
 * Headings are rows in their own right so that a whole run can be dragged or
 * removed as one, the way the Apple apps' queue works.
 */
sealed interface QueueRow {
    val key: String

    /** The queue items this row stands for, in queue order. */
    val itemIds: List<String>

    /** What the row is called, for saying what was removed. */
    val label: String

    /** The queue items themselves: a heading's run, or the one track. */
    val items: List<QueueItem>

    data class Album(val group: QueueGroup) : QueueRow {
        override val key get() = "album:${group.items.first().queueItemId}"
        override val itemIds get() = group.items.map { it.queueItemId }
        override val label get() = group.title
        override val items get() = group.items
    }

    data class Track(val item: QueueItem) : QueueRow {
        override val key get() = item.queueItemId
        override val itemIds get() = listOf(item.queueItemId)
        override val label get() = item.title
        override val items get() = listOf(item)
    }

    data class Single(val item: QueueItem) : QueueRow {
        override val key get() = item.queueItemId
        override val itemIds get() = listOf(item.queueItemId)
        override val label get() = item.title
        override val items get() = listOf(item)
    }

    companion object {
        /**
         * Contiguous runs, as the TUI and the Apple apps group them: queue
         * order is the listener's, and collapsing two separate visits to the
         * same record into one heading would misrepresent it. Tracks with no
         * album stand alone; a run of one whose title is the record's is a
         * single.
         */
        fun build(queue: List<QueueItem>, grouped: Boolean): List<QueueRow> {
            if (!grouped) return queue.map(::Track)
            val rows = ArrayList<QueueRow>(queue.size + queue.size / 8)
            var index = 0
            while (index < queue.size) {
                val first = queue[index]
                if (first.album.isEmpty()) {
                    rows += Track(first)
                    index++
                    continue
                }
                var end = index + 1
                while (end < queue.size && queue[end].album == first.album && queue[end].albumArtist == first.albumArtist) end++
                val run = queue.subList(index, end)
                if (run.size == 1 && first.title.trim().equals(first.album.trim(), ignoreCase = true)) {
                    rows += Single(first)
                } else {
                    rows += Album(QueueGroup(first.albumArtist, first.album, run.toList()))
                    run.mapTo(rows, ::Track)
                }
                index = end
            }
            return rows
        }

        /** The queue items a set of row keys stands for; a heading stands for its run. */
        fun itemIds(keys: Set<String>, rows: List<QueueRow>) = rows.filter { it.key in keys }.flatMap { it.itemIds }.distinct()
    }
}

data class QueueGroup(val albumArtist: String, val album: String, val items: List<QueueItem>) {
    val title get() = album.ifEmpty { albumArtist.ifEmpty { "Unknown Artist" } }

    /** "2007 · 11 tracks · 59:10 · FLAC". The codec only when the whole run shares one. */
    val detail: String
        get() {
            val parts = mutableListOf<String>()
            items.first().year?.takeIf { it.isNotEmpty() }?.let(parts::add)
            parts += count(items.size, "track")
            val total = items.sumOf { it.durationMs?.toLong() ?: 0 }
            if (total > 0) parts += duration(total)
            items.mapNotNull { it.codec }.toSet().singleOrNull()?.let { parts += it.uppercase() }
            return parts.joinToString(" · ")
        }
}

/**
 * Where to move `moving` so that it lands where a drag left it in `dropped`,
 * the rows as drawn at the end of the drag: before the first row after it that
 * is not itself moving, else after the last item that stays.
 *
 * Anchoring to the row below rather than the row above is what lets a drop
 * say "at the very top", and lands a drop on a heading above its record.
 */
fun moveTarget(dropped: List<QueueRow>, movedKey: String, moving: Set<String>, queue: List<QueueItem>): Pair<String, Boolean>? {
    val at = dropped.indexOfFirst { it.key == movedKey }
    if (at < 0) return null
    dropped.drop(at + 1).firstNotNullOfOrNull { row -> row.itemIds.firstOrNull { it !in moving } }?.let { return it to false }
    return queue.lastOrNull { it.queueItemId !in moving }?.let { it.queueItemId to true }
}

fun count(n: Int, noun: String) = "%,d %s".format(n, if (n == 1) noun else "${noun}s")

/** 59:10, or 3:02:05 once it runs past an hour. */
fun duration(ms: Long): String {
    val s = ms.coerceAtLeast(0) / 1000
    val h = s / 3600
    return if (h > 0) "%d:%02d:%02d".format(h, s / 60 % 60, s % 60) else "%d:%02d".format(s / 60, s % 60)
}
