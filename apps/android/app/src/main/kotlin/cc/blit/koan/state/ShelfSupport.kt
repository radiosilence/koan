package cc.blit.koan.state

import uniffi.koan_ffi.ShelfSummary

/**
 * How many of each kind a query or shelf finds in the library as its browsers
 * list them, for each section's heading. The previews are the head of those
 * listings, so these can be more than the previews hold.
 */
data class ShelfTotals(val artists: Long, val albums: Long, val tracks: Long) {
    companion object {
        fun of(summary: ShelfSummary) =
            ShelfTotals(summary.artistTotal.toLong(), summary.albumTotal.toLong(), summary.trackTotal.toLong())
    }

    /** Only the kinds there are, so a tracks-only library reads as a count of tracks. */
    fun text(): String = listOfNotNull(
        count(artists, "artist"),
        count(albums, "album"),
        count(tracks, "track"),
    ).joinToString(" · ")

    private fun count(n: Long, noun: String): String? = if (n > 0) "$n $noun${if (n == 1L) "" else "s"}" else null
}

/**
 * A section's heading number: how many the library has (the preview's own
 * count until that is known). The number the heading's "See all" listing will
 * show, so the two agree.
 */
fun headingCount(total: Long?, shown: Int): Long = total ?: shown.toLong()

/**
 * How many record tiles go across: the fewest that keep each within [max],
 * as long as each stays at least [min]. The shelf's preview is cut into rows
 * of this many, because a lazy list recycles rows, not the cells of a grid
 * inside one row. All in dp.
 */
fun tileColumns(width: Float, min: Float = 140f, max: Float = 190f, spacing: Float = 16f): Int {
    val usable = width + spacing
    val most = maxOf(1, (usable / (min + spacing)).toInt())
    val fewest = maxOf(1, Math.ceil((usable / (max + spacing)).toDouble()).toInt())
    return minOf(most, fewest)
}

/** [items] cut into rows of [perRow]; a short last row is kept. */
fun <T> rowsOf(items: List<T>, perRow: Int): List<List<T>> = items.chunked(maxOf(1, perRow))
