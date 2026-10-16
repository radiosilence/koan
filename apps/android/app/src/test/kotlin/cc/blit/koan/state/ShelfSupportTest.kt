package cc.blit.koan.state

import cc.blit.koan.FakeEngine
import cc.blit.koan.nav.Section
import cc.blit.koan.ui.pages.shelfDuration
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.koan_ffi.ShelfKind
import uniffi.koan_ffi.ShelfSummary

@OptIn(ExperimentalCoroutinesApi::class)
class ShelfSupportTest {
    @Test
    fun `counts name only the kinds there are, from the shelf's totals`() {
        val summary = ShelfSummary(emptyList(), 1uL, emptyList(), 0uL, emptyList(), 240uL)
        assertEquals("1 artist · 240 tracks", ShelfTotals.of(summary).text())
        assertEquals("", ShelfTotals(0, 0, 0).text())
    }

    @Test
    fun `a heading says the listing's length, the preview's until that is known`() {
        // A preview of 12 records out of 300: the heading says 300, and so does the listing it opens.
        assertEquals(300L, headingCount(300, shown = 12))
        assertEquals(12L, headingCount(null, shown = 12))
    }

    @Test
    fun `tiles go across within the grids' sizes`() {
        // 360 dp phone with 22 dp gutters: 316 usable, two tiles of 150.
        assertEquals(2, tileColumns(316f))
        assertEquals(1, tileColumns(120f))
        // A tablet: as many as keep each within 190.
        val wide = tileColumns(1000f)
        val tile = (1000f - 16f * (wide - 1)) / wide
        assertTrue("tile $tile", tile in 140f..190f)
    }

    @Test
    fun `rows keep a short last row`() {
        assertEquals(listOf(listOf(1, 2, 3), listOf(4, 5)), rowsOf(listOf(1, 2, 3, 4, 5), 3))
        assertEquals(listOf(listOf(1), listOf(2)), rowsOf(listOf(1, 2), 0))
    }

    @Test
    fun `see all opens the listing the preview is the head of`() = runTest {
        val library = LibraryModel(FakeEngine().engine, EngineMirror(), backgroundScope)
        for ((shelf, check) in listOf<Pair<ShelfKind, (uniffi.koan_ffi.BrowseFilter) -> Boolean>>(
            ShelfKind.Favourites to { it.favourites },
            ShelfKind.Recent to { it.recent },
            ShelfKind.Downloaded to { it.downloaded },
        )) {
            assertEquals(Section.Albums, library.browse(ShelfList.Albums, shelf))
            assertTrue("$shelf narrows the browser", check(library.browseFilter.value))
        }
        assertEquals(Section.Tracks, library.browse(ShelfList.Tracks, ShelfKind.Search("miles")))
        assertEquals(Section.Artists, library.browse(ShelfList.Artists, ShelfKind.Favourites))
    }

    @Test
    fun `durations read as clocks`() {
        assertEquals("3:05", shelfDuration(185_000))
        assertEquals("1:02:03", shelfDuration(3_723_000))
        assertEquals("", shelfDuration(null))
    }
}
