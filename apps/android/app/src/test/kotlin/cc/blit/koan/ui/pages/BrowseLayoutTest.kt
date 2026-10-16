package cc.blit.koan.ui.pages

import cc.blit.koan.nav.Section
import cc.blit.koan.nav.Tab
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class BrowseLayoutTest {
    @Test
    fun `a narrow phone gets two columns of at least the minimum, never fewer than one`() {
        // 360dp phone at 2.75x: 328dp of grid, 14dp gaps, 140dp tiles.
        assertEquals(2, tileColumnCount(902, 38, 385))
        assertEquals(1, tileColumnCount(100, 38, 385))
        assertEquals(1, tileColumnCount(0, 10, 100))
        assertEquals(4, tileColumnCount(1000, 10, 220))
    }

    @Test
    fun `tile widths fill the width exactly and differ by at most a pixel`() {
        for (avail in listOf(902, 903, 1000, 1079, 1440)) {
            val widths = tileWidths(avail, 38, 385)
            assertEquals(avail, widths.sum() + 38 * (widths.size - 1))
            assertTrue(widths.max() - widths.min() <= 1)
            assertTrue("every tile is at least the minimum", widths.min() >= 385)
        }
    }

    @Test
    fun `a scroll place is kept per tab and section, and top is not kept`() {
        val store = ScrollStore()
        assertEquals(ScrollPlace.Top, store[Tab.Library, Section.Albums])
        store.put(Tab.Library, Section.Albums, ScrollPlace(40, 12))
        store.put(Tab.Search, Section.Albums, ScrollPlace(2, 0))
        store.put(Tab.Library, Section.Artists, ScrollPlace(7, 3))
        assertEquals(ScrollPlace(40, 12), store[Tab.Library, Section.Albums])
        assertEquals(ScrollPlace(2, 0), store[Tab.Search, Section.Albums])
        assertEquals(ScrollPlace(7, 3), store[Tab.Library, Section.Artists])
        store.put(Tab.Library, Section.Albums, ScrollPlace.Top)
        assertEquals(ScrollPlace.Top, store[Tab.Library, Section.Albums])
        assertEquals(ScrollPlace(2, 0), store[Tab.Search, Section.Albums])
    }

    @Test
    fun `every section the library lists has a title and a glyph in the set`() {
        for (section in Section.library) {
            assertTrue(section.title.isNotEmpty())
            assertTrue("${section.title}: ${section.iconName}", cc.blit.koan.theme.KoanGlyph.names.contains(section.iconName))
        }
        assertEquals("Recently Played", Section.RecentlyPlayed.title)
    }

    @Test
    fun `counts pluralise and group thousands, and durations read as clocks`() {
        assertEquals("1 album", countLabel(1, "album"))
        assertEquals("0 albums", countLabel(0, "album"))
        assertEquals("2,314 tracks", countLabel(2314, "track"))
        assertEquals("3:07", durationLabel(187_000))
        assertEquals("0:05", durationLabel(5_400))
        assertEquals("1:02:09", durationLabel(3_729_000))
        assertEquals("", durationLabel(null))
        assertEquals("", durationLabel(0))
    }

    @Test
    fun `a year applies only when whole or emptied, and keeps four digits`() {
        assertEquals(YearTyping("", true, null), yearTyped(""))
        assertEquals(YearTyping("19", false, null), yearTyped("19"))
        assertEquals(YearTyping("1990", true, 1990), yearTyped("1990"))
        assertEquals(YearTyping("1990", true, 1990), yearTyped("19x9y0z12"))
        assertNull(yearTyped("abc").value)
    }

    @Test
    fun `a filter that is on is offered even when the library no longer has it`() {
        assertEquals(listOf("FLAC", "MP3"), offeredChoices(listOf("FLAC", "MP3"), "MP3"))
        assertEquals(listOf("FLAC", "OPUS"), offeredChoices(listOf("FLAC"), "OPUS"))
        assertEquals(listOf("FLAC"), offeredChoices(listOf("FLAC"), null))
        assertEquals(emptyList<String>(), offeredChoices(null, null))
    }
}
