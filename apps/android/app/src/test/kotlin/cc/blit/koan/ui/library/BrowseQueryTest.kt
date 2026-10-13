package cc.blit.koan.ui.library

import cc.blit.koan.ffi.AlbumSort
import cc.blit.koan.ffi.TrackBrowseSort
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class BrowseQueryTest {
    @Test
    fun shelvesOpenTheirBrowsersOnTheirOwnFilterAndOrder() {
        assertEquals(BrowseQuery(AlbumSort.TITLE, NoFilter), null.albumQuery(AlbumSort.TITLE))
        assertEquals(BrowseQuery(AlbumSort.ARTIST, NoFilter.copy(favourites = true)), Shelf.Favourites.albumQuery(AlbumSort.TITLE))
        assertEquals(BrowseQuery(AlbumSort.LAST_PLAYED, NoFilter.copy(recent = true)), Shelf.Recent.albumQuery(AlbumSort.TITLE))
        assertEquals(BrowseQuery(AlbumSort.DOWNLOADED, NoFilter.copy(downloaded = true)), Shelf.Downloaded.albumQuery(AlbumSort.TITLE))
        assertEquals(BrowseQuery(AlbumSort.BEST_MATCH, NoFilter, "moth"), Shelf.Search("moth").albumQuery(AlbumSort.TITLE))
        assertEquals(TrackBrowseSort.LAST_PLAYED, Shelf.Recent.trackQuery().sort)
    }

    @Test
    fun sortsAreOfferedOnlyWhereTheyMeanSomething() {
        val plain = BrowseQuery(AlbumSort.TITLE, NoFilter).offered()
        assertFalse(AlbumSort.LAST_PLAYED in plain)
        assertFalse(AlbumSort.BEST_MATCH in plain)
        assertFalse(AlbumSort.DOWNLOADED in plain)
        val narrowed = BrowseQuery(AlbumSort.TITLE, NoFilter.copy(recent = true, downloaded = true), "x").offered()
        assertEquals(listOf(AlbumSort.BEST_MATCH, AlbumSort.LAST_PLAYED, AlbumSort.DOWNLOADED), narrowed.take(3))
        assertTrue(TrackBrowseSort.LAST_PLAYED in BrowseQuery(TrackBrowseSort.ARTIST, NoFilter.copy(recent = true)).offered())
    }

    @Test
    fun theYearRangeCountsAsOneFilter() {
        assertEquals(0, NoFilter.activeCount)
        assertEquals(2, NoFilter.copy(lossless = true, yearFrom = 1990, yearTo = 1999).activeCount)
    }

    @Test
    fun lengthsRead() {
        assertEquals("3:07", duration(187_000))
        assertEquals("1:02:45", duration(3_765_000))
        assertEquals("42 min", length(42 * 60_000))
        assertEquals("1 h 12 min", length(72 * 60_000))
        assertEquals("1 album", count(1, "album"))
        assertEquals("3 albums", count(3, "album"))
    }
}
