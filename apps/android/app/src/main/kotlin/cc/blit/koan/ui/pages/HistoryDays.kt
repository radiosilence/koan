package cc.blit.koan.ui.pages

import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import java.util.Locale

/** One row of the history list: a day's heading, or a play under it. */
sealed interface HistoryLine {
    /** Stable across list edits; a heading's is its run, a play's is the play. */
    val key: String

    data class Heading(val run: Int, val day: LocalDate) : HistoryLine {
        override val key: String get() = "day:$run"
    }

    data class Play<T>(val id: Long, val item: T) : HistoryLine {
        override val key: String get() = "play:$id"
    }
}

/**
 * Day grouping for the play history, from `HistoryView.swift`: runs of
 * consecutive entries that fall on the same calendar day in the viewer's zone.
 * The list is already ordered by time, so one pass is enough and nothing is
 * sorted; two visits to the same day cannot be separated, they get a heading
 * each.
 *
 * Days are cut at local midnight with `java.time`, never by dividing seconds
 * by 86 400, so a day with 23 or 25 hours (daylight saving) groups as the
 * wall clock does.
 */
object HistoryDays {
    fun dayOf(epochSeconds: Long, zone: ZoneId): LocalDate =
        Instant.ofEpochSecond(epochSeconds).atZone(zone).toLocalDate()

    /** Headings and plays interleaved, in the order given. */
    fun <T> lines(items: List<T>, id: (T) -> Long, playedAt: (T) -> Long, zone: ZoneId): List<HistoryLine> {
        val out = ArrayList<HistoryLine>(items.size + 8)
        var last: LocalDate? = null
        var run = 0
        for (item in items) {
            val day = dayOf(playedAt(item), zone)
            if (day != last) {
                out += HistoryLine.Heading(run++, day)
                last = day
            }
            out += HistoryLine.Play(id(item), item)
        }
        return out
    }

    /** "Today", "Yesterday", then the full date ("Tuesday, 14 October 2025"), as `doesRelativeDateFormatting` gives. */
    fun heading(day: LocalDate, today: LocalDate, locale: Locale): String = when (day) {
        today -> "Today"
        today.minusDays(1) -> "Yesterday"
        else -> DateTimeFormatter.ofLocalizedDate(FormatStyle.FULL).withLocale(locale).format(day)
    }

    /** The time of day a play started, in the short style of the locale. */
    fun time(epochSeconds: Long, zone: ZoneId, locale: Locale): String =
        DateTimeFormatter.ofLocalizedTime(FormatStyle.SHORT).withLocale(locale)
            .format(Instant.ofEpochSecond(epochSeconds).atZone(zone))
}

/** What is ticked in the history list. By play id, never track id: the same track can appear many times. */
object HistorySelection {
    fun toggle(selection: Set<Long>, id: Long): Set<Long> = if (id in selection) selection - id else selection + id

    /** Selection survives edits only for plays still in the list (a forgotten play, or a filter that hides it, drops out). */
    fun retain(selection: Set<Long>, visible: Collection<Long>): Set<Long> {
        if (selection.isEmpty()) return selection
        val live = visible.toHashSet()
        return selection.filterTo(HashSet()) { it in live }
    }
}

/** "3:07", or "1:02:03" past an hour. */
fun historyDuration(ms: Long): String {
    val total = (ms.coerceAtLeast(0) / 1000)
    val h = total / 3600
    val m = (total % 3600) / 60
    val s = total % 60
    return if (h > 0) "%d:%02d:%02d".format(h, m, s) else "%d:%02d".format(m, s)
}
