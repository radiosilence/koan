package cc.blit.koan.ui

import cc.blit.koan.ui.history.byDay
import cc.blit.koan.ui.history.dayLabel
import cc.blit.koan.ui.playlists.duration
import cc.blit.koan.ui.playlists.runs
import java.time.LocalDate
import java.time.ZoneOffset
import org.junit.Assert.assertEquals
import org.junit.Test

class CollectionsTest {
    @Test
    fun runsKeepSeparateVisitsToARecordApart() {
        val records = listOf("a", "a", "b", "a", "c", "c")
        assertEquals(
            listOf(listOf("a", "a"), listOf("b"), listOf("a"), listOf("c", "c")),
            runs(records) { x, y -> x == y },
        )
    }

    @Test
    fun runsOfNothingIsNothing() {
        assertEquals(emptyList<List<String>>(), runs(emptyList<String>()) { _, _ -> true })
    }

    @Test
    fun playsGroupByTheirLocalDay() {
        val zone = ZoneOffset.ofHours(2)
        // 2025-10-13 00:30, then 2025-10-12 23:30 and 23:20 at +02:00, newest first.
        val plays = listOf(1_760_308_200L, 1_760_304_600L, 1_760_304_000L)
        val days = byDay(plays, zone) { it }
        assertEquals(listOf(LocalDate.of(2025, 10, 13), LocalDate.of(2025, 10, 12)), days.map { it.first })
        assertEquals(listOf(1, 2), days.map { it.second.size })
    }

    @Test
    fun recentDaysAreNamed() {
        val today = LocalDate.of(2026, 10, 13)
        assertEquals("Today", dayLabel(today, today))
        assertEquals("Yesterday", dayLabel(today.minusDays(1), today))
    }

    @Test
    fun durationsRoundToTheMinute() {
        assertEquals("48 min", duration(48 * 60_000L + 10_000))
        assertEquals("2 h 5 min", duration(125 * 60_000L))
    }
}
