package cc.blit.koan.state

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import uniffi.koan_ffi.AlbumSort
import uniffi.koan_ffi.TrackBrowseSort

class BrowseSupportTest {
    @Test
    fun `a filter survives being stored, awkward strings included`() {
        val filter = BrowseFilterNone.copy(
            favourites = true, lossless = true, codec = "FLAC", genre = "Drum & Bass\nJungle = \\", yearFrom = 1990, yearTo = 1999,
        )
        assertEquals(filter, browseFilterOf(filter.stored()))
        assertEquals(BrowseFilterNone, browseFilterOf(BrowseFilterNone.stored()))
        assertEquals(BrowseFilterNone, browseFilterOf(null))
        assertEquals(BrowseFilterNone, browseFilterOf("garbage\n=x\nyearFrom=nope"))
    }

    @Test
    fun `the year range counts as one filter`() {
        assertEquals(0, BrowseFilterNone.activeCount)
        assertEquals(1, BrowseFilterNone.copy(yearFrom = 1990, yearTo = 1999).activeCount)
        assertEquals(3, BrowseFilterNone.copy(recent = true, codec = "MP3", yearTo = 2000).activeCount)
    }

    @Test
    fun `every sort has a stable key that finds it again`() {
        for (sort in AlbumSort.entries) assertEquals(sort, albumSortOf(sort.storageKey))
        for (sort in TrackBrowseSort.entries) assertEquals(sort, trackSortOf(sort.storageKey))
        assertEquals(AlbumSort.entries.size, AlbumSort.entries.map { it.storageKey }.toSet().size)
        assertEquals(TrackBrowseSort.entries.size, TrackBrowseSort.entries.map { it.storageKey }.toSet().size)
        assertNull(albumSortOf("nonsense"))
        assertNull(trackSortOf(null))
    }

    @Test
    fun `the sorts offered depend on which filters are on`() {
        assertEquals(
            listOf(AlbumSort.RECENTLY_ADDED, AlbumSort.TITLE, AlbumSort.ARTIST, AlbumSort.YEAR, AlbumSort.RANDOM),
            offeredAlbumSorts(recent = false, downloaded = false, searching = false),
        )
        assertEquals(
            listOf(AlbumSort.BEST_MATCH, AlbumSort.LAST_PLAYED, AlbumSort.DOWNLOADED),
            offeredAlbumSorts(recent = true, downloaded = true, searching = true).take(3),
        )
        assertEquals(
            listOf(TrackBrowseSort.BEST_MATCH, TrackBrowseSort.LAST_PLAYED, TrackBrowseSort.ARTIST),
            offeredTrackSorts(recent = true, searching = true).take(3),
        )
    }
}
