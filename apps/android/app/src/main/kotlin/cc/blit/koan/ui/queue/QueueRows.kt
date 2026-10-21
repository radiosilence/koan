package cc.blit.koan.ui.queue

import cc.blit.koan.ffi.QueueItem
import cc.blit.koan.ffi.RunRole

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

        /** The records the run is of: one, unless editions share its tags. */
        val albumIds get() = group.items.mapNotNull { it.albumId }.distinct()
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
         * The queue as the engine grouped it: a heading above each row that
         * opens a record's run, the run's rows beneath it, and rows on no
         * record or of a single standing alone.
         */
        fun build(queue: List<QueueItem>, grouped: Boolean): List<QueueRow> {
            if (!grouped) return queue.map(::Track)
            val rows = ArrayList<QueueRow>(queue.size + queue.size / 8)
            var index = 0
            while (index < queue.size) {
                val first = queue[index]
                when (first.run) {
                    RunRole.ALONE, RunRole.CONTINUES -> {
                        rows += Track(first)
                        index++
                    }
                    RunRole.SINGLE -> {
                        rows += Single(first)
                        index++
                    }
                    RunRole.OPENS -> {
                        var end = index + 1
                        while (end < queue.size && queue[end].run == RunRole.CONTINUES) end++
                        val run = queue.subList(index, end).toList()
                        rows += Album(QueueGroup(first.album, run))
                        run.mapTo(rows, ::Track)
                        index = end
                    }
                }
            }
            return rows
        }

        /** The queue items a set of row keys stands for; a heading stands for its run. */
        fun itemIds(keys: Set<String>, rows: List<QueueRow>) = rows.filter { it.key in keys }.flatMap { it.itemIds }.distinct()
    }
}

data class QueueGroup(val album: String, val items: List<QueueItem>) {
    val title get() = album

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
