package cc.blit.koan.ui.pages

import org.junit.Assert.assertEquals
import org.junit.Test
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.ZoneId
import java.util.Locale

class HistoryDaysTest {
    private val london = ZoneId.of("Europe/London")
    private val tokyo = ZoneId.of("Asia/Tokyo")

    private fun at(zone: ZoneId, y: Int, mo: Int, d: Int, h: Int, mi: Int = 0) =
        LocalDateTime.of(y, mo, d, h, mi).atZone(zone).toEpochSecond()

    private fun days(times: List<Long>, zone: ZoneId): List<String> =
        HistoryDays.lines(times.indices.toList(), { it.toLong() }, { times[it] }, zone)
            .map { if (it is HistoryLine.Heading) "H${it.day}" else "P" }

    @Test
    fun `plays on one day share a heading and a new day starts another`() {
        val times = listOf(
            at(london, 2025, 6, 3, 23, 50), at(london, 2025, 6, 3, 9), at(london, 2025, 6, 2, 22),
        )
        assertEquals(listOf("H2025-06-03", "P", "P", "H2025-06-02", "P"), days(times, london))
    }

    @Test
    fun `the same instant is a different day in a different zone`() {
        val t = at(london, 2025, 6, 3, 23, 30) // 07:30 next day in Tokyo
        assertEquals(LocalDate.of(2025, 6, 3), HistoryDays.dayOf(t, london))
        assertEquals(LocalDate.of(2025, 6, 4), HistoryDays.dayOf(t, tokyo))
    }

    @Test
    fun `the spring-forward day of 23 hours groups by the wall clock`() {
        // Europe/London springs forward on 2025-03-30 at 01:00 UTC, so the day is 23 hours long.
        val first = at(london, 2025, 3, 30, 0, 10)
        val last = at(london, 2025, 3, 30, 23, 50)
        val next = at(london, 2025, 3, 31, 0, 5)
        assertEquals(listOf("H2025-03-31", "P", "H2025-03-30", "P", "P"), days(listOf(next, last, first), london))
    }

    @Test
    fun `the fall-back day of 25 hours is still one day`() {
        val first = at(london, 2025, 10, 26, 0, 10)
        val last = at(london, 2025, 10, 26, 23, 50)
        assertEquals(listOf("H2025-10-26", "P", "P"), days(listOf(last, first), london))
    }

    @Test
    fun `non-consecutive visits to a day get a heading each, with distinct keys`() {
        val times = listOf(at(london, 2025, 6, 3, 12), at(london, 2025, 6, 2, 12), at(london, 2025, 6, 3, 8))
        val lines = HistoryDays.lines(times.indices.toList(), { it.toLong() }, { times[it] }, london)
        assertEquals(lines.size, lines.map { it.key }.toSet().size)
        assertEquals(3, lines.count { it is HistoryLine.Heading })
    }

    @Test
    fun `empty history has no lines`() {
        assertEquals(emptyList<HistoryLine>(), HistoryDays.lines(emptyList<Long>(), { it }, { it }, london))
    }

    @Test
    fun `today and yesterday are named and older days are dated`() {
        val today = LocalDate.of(2025, 6, 3)
        assertEquals("Today", HistoryDays.heading(today, today, Locale.UK))
        assertEquals("Yesterday", HistoryDays.heading(today.minusDays(1), today, Locale.UK))
        assertEquals("Sunday, 1 June 2025", HistoryDays.heading(LocalDate.of(2025, 6, 1), today, Locale.UK))
    }

    @Test
    fun `time is the short style in the zone given`() {
        val t = at(london, 2025, 6, 3, 14, 5)
        assertEquals("14:05", HistoryDays.time(t, london, Locale.UK))
        assertEquals("22:05", HistoryDays.time(t, tokyo, Locale.UK))
    }

    @Test
    fun `selection toggles and drops plays that are gone`() {
        var s = emptySet<Long>()
        s = HistorySelection.toggle(s, 4)
        s = HistorySelection.toggle(s, 9)
        assertEquals(setOf(4L, 9L), s)
        assertEquals(setOf(9L), HistorySelection.toggle(s, 4))
        assertEquals(setOf(9L), HistorySelection.retain(s, listOf(9, 10)))
        assertEquals(emptySet<Long>(), HistorySelection.retain(s, emptyList()))
    }

    @Test
    fun `durations read m colon ss and past an hour h colon mm colon ss`() {
        assertEquals("0:00", formatDuration(-5))
        assertEquals("3:07", formatDuration(187_400))
        assertEquals("1:02:03", formatDuration(3_723_000))
    }
}
