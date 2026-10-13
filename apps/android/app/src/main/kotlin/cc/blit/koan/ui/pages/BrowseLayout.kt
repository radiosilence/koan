package cc.blit.koan.ui.pages

import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.Dp
import cc.blit.koan.nav.Section
import cc.blit.koan.nav.Tab
import java.util.Locale

/*
 * What the browsers decide without drawing: how many tiles fit, where each
 * tab's lists were left, what a count and a duration say, and how a typed
 * year becomes a filter. Plain Kotlin, so a JVM test runs it.
 */

/**
 * How many tiles of at least [minPx] fit across [availablePx] with [spacingPx]
 * between them: as `GridItem.adaptive(minimum:)` answers on the Apple apps,
 * and never fewer than one.
 */
fun tileColumnCount(availablePx: Int, spacingPx: Int, minPx: Int): Int =
    maxOf(1, (availablePx + spacingPx) / (minPx + spacingPx))

/** The widths of those tiles. They share the width out whole, so a row ends exactly at the edge. */
fun tileWidths(availablePx: Int, spacingPx: Int, minPx: Int): List<Int> {
    val n = tileColumnCount(availablePx, spacingPx, minPx)
    val total = maxOf(0, availablePx - spacingPx * (n - 1))
    val base = total / n
    val extra = total % n
    return List(n) { base + if (it < extra) 1 else 0 }
}

/** The album grid's columns: as many tiles of at least [min] as fit. */
data class TileCells(val min: Dp) : GridCells {
    override fun Density.calculateCrossAxisCellSizes(availableSize: Int, spacing: Int): List<Int> =
        tileWidths(availableSize, spacing, min.roundToPx())
}

/** Where a list was left: the first row showing, and how far into it. */
data class ScrollPlace(val index: Int, val offset: Int) {
    companion object {
        val Top = ScrollPlace(0, 0)
    }
}

/**
 * Where each tab left each listing. A browser is rebuilt on each visit (a
 * tab's stack keeps the route, not the composition), so what it was scrolled
 * to is held here and put back. Keyed by tab as well as section: the same
 * listing opened from two tabs is two places.
 */
class ScrollStore {
    private val places = HashMap<Pair<Tab, Section>, ScrollPlace>()

    operator fun get(tab: Tab, section: Section): ScrollPlace = synchronized(places) { places[tab to section] ?: ScrollPlace.Top }

    fun put(tab: Tab, section: Section, place: ScrollPlace) {
        synchronized(places) {
            if (place == ScrollPlace.Top) places.remove(tab to section) else places[tab to section] = place
        }
    }

    companion object {
        /** The app's: one for the process, because the tabs outlive any one composition. */
        val App = ScrollStore()
    }
}

/** What a section is called on the Library page and over its listing. */
val Section.title: String
    get() = when (this) {
        Section.Queue -> "Queue"
        Section.SearchResults -> "Search"
        Section.Albums -> "Albums"
        Section.Artists -> "Artists"
        Section.Tracks -> "Tracks"
        Section.Favourites -> "Favourites"
        Section.RecentlyPlayed -> "Recently Played"
        Section.OnDevice -> "On Device"
        Section.PlayHistory -> "History"
        Section.Downloads -> "Downloads"
        is Section.Playlist -> "Playlist"
    }

/** The glyph for a section, by name in the glyph set. */
val Section.iconName: String
    get() = when (this) {
        Section.Queue -> "list"
        Section.SearchResults -> "search"
        Section.Albums -> "album"
        Section.Artists -> "artist"
        Section.Tracks -> "track"
        Section.Favourites -> "heart"
        Section.RecentlyPlayed -> "clock"
        Section.OnDevice -> "phone"
        Section.PlayHistory -> "history"
        Section.Downloads -> "download"
        is Section.Playlist -> "playlist"
    }

/** "1 album", "2,314 albums". */
fun countLabel(n: Long, noun: String): String =
    String.format(Locale.US, "%,d", n) + " " + if (n == 1L) noun else noun + "s"

/** "3:07", or "1:02:09" from an hour up; empty when the length is unknown. */
fun durationLabel(ms: Long?): String {
    if (ms == null || ms <= 0) return ""
    val total = ms / 1000
    val h = total / 3600
    val m = total % 3600 / 60
    val s = total % 60
    return if (h > 0) String.format(Locale.US, "%d:%02d:%02d", h, m, s) else String.format(Locale.US, "%d:%02d", m, s)
}

/** A year box after a keystroke: the digits kept, and what, if anything, it sets the filter to. */
data class YearTyping(
    val text: String,
    /** Whether the filter should be set to [value] now: a whole year, or an emptied field. */
    val apply: Boolean,
    val value: Int?,
)

/**
 * Applied as it is typed rather than on Return: only a whole year or an empty
 * field is, so typing 1990 is one query rather than four.
 */
fun yearTyped(typed: String): YearTyping {
    val digits = typed.filter { it in '0'..'9' }.take(4)
    return when {
        digits.isEmpty() -> YearTyping(digits, true, null)
        digits.length == 4 -> YearTyping(digits, true, digits.toInt())
        else -> YearTyping(digits, false, null)
    }
}

/** The codec and genre choices, with the one on kept even when the library no longer has it. */
fun offeredChoices(choices: List<String>?, current: String?): List<String> {
    val list = choices.orEmpty()
    return if (current == null || current in list) list else list + current
}
