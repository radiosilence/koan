package cc.blit.koan.state

import cc.blit.koan.FakeEngine
import cc.blit.koan.album
import cc.blit.koan.artist
import cc.blit.koan.nav.Route
import cc.blit.koan.nav.Section
import cc.blit.koan.record
import cc.blit.koan.track
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.koan_ffi.AlbumPage
import uniffi.koan_ffi.AlbumSort
import uniffi.koan_ffi.KoanException
import uniffi.koan_ffi.ShelfKind
import uniffi.koan_ffi.ShelfSummary
import uniffi.koan_ffi.StateSlice
import uniffi.koan_ffi.TrackBrowseSort
import uniffi.koan_ffi.TrackListing

@OptIn(ExperimentalCoroutinesApi::class)
class LibraryModelTest {
    private val mirror = EngineMirror()

    private fun TestScope.model(
        engine: FakeEngine,
        store: KeyValueStore = KeyValueStore.Memory(),
        activity: ActivityModel? = null,
    ) = LibraryModel(engine.engine, mirror, backgroundScope, store, activity, art = null, filterDebounceMs = 80)

    private fun FakeEngine.count(prefix: String) = synchronized(calls) { calls.count { it.startsWith(prefix) } }

    /** Moves the model onto a section the way the navigator does. */
    private suspend fun LibraryModel.moveTo(section: Section) {
        prepare(section)?.let { show(it) }
    }

    @Test
    fun `a section is read before it is shown, and not again while it is showing`() = runTest {
        val engine = FakeEngine { name, _ -> if (name == "albums") listOf(album(1), album(2)) else null }
        val model = model(engine)
        assertEquals(emptyList<Any>(), model.visibleAlbums.value)

        val listing = model.prepare(Section.Albums)
        // Nothing on screen has moved yet: the rows are in hand, the section is not adopted.
        assertNotNull(listing)
        assertEquals(Section.Queue, model.section.value)
        assertEquals(emptyList<Any>(), model.visibleAlbums.value)

        model.show(listing!!)
        runCurrent()
        assertEquals(Section.Albums, model.section.value)
        assertEquals(listOf(album(1), album(2)), model.visibleAlbums.value)
        // Already showing: a move back onto it is not a reason to re-read it.
        assertNull(model.prepare(Section.Albums))
        assertEquals(1, engine.count("albums("))
    }

    @Test
    fun `the same answer as last time wakes nobody`() = runTest {
        // A fresh list each time, equal by value: the usual answer to a library change.
        val engine = FakeEngine { name, _ -> if (name == "albums") listOf(album(1), album(2)) else null }
        val model = model(engine)
        model.moveTo(Section.Albums)
        val seen = model.visibleAlbums.record(backgroundScope)
        assertEquals(1, seen.size)
        model.libraryChanged()
        runCurrent()
        assertTrue("it asked again", engine.count("albums(") >= 2)
        assertEquals("but published nothing", 1, seen.size)
    }

    @Test
    fun `a different answer is published`() = runTest {
        var answer = listOf(album(1))
        val engine = FakeEngine { name, _ -> if (name == "albums") answer else null }
        val model = model(engine)
        model.moveTo(Section.Albums)
        val seen = model.visibleAlbums.record(backgroundScope)
        answer = listOf(album(1), album(2))
        model.libraryChanged()
        runCurrent()
        assertEquals(2, seen.size)
        assertEquals(listOf(album(1), album(2)), seen.last())
    }

    @Test
    fun `typing is one query, after the debounce, with the last text`() = runTest {
        val engine = FakeEngine { name, _ -> if (name == "albums") emptyList<Any>() else null }
        val model = model(engine)
        model.moveTo(Section.Albums)
        val before = engine.count("albums(")
        model.setFilter("a"); runCurrent()
        advanceTimeBy(30)
        model.setFilter("ab"); runCurrent()
        advanceTimeBy(30)
        model.setFilter("abc"); runCurrent()
        advanceTimeBy(79)
        assertEquals("nothing yet", before, engine.count("albums("))
        advanceTimeBy(2); runCurrent()
        assertEquals(before + 1, engine.count("albums("))
        assertTrue(engine.calls.last { it.startsWith("albums(") }.contains(", abc, "))
        assertTrue(model.isNarrowed)
    }

    @Test
    fun `an empty filter is no search at all`() = runTest {
        val engine = FakeEngine { name, _ -> if (name == "artists") emptyList<Any>() else null }
        val model = model(engine)
        model.moveTo(Section.Artists)
        assertTrue(engine.calls.last { it.startsWith("artists(") }.startsWith("artists(null, "))
    }

    @Test
    fun `sorts and filters persist, reload the browser, and come back on the next launch`() = runTest {
        val store = KeyValueStore.Memory()
        val engine = FakeEngine { name, _ -> if (name == "albums") emptyList<Any>() else null }
        val model = model(engine, store)
        model.moveTo(Section.Albums)
        model.setAlbumSort(AlbumSort.TITLE)
        model.setBrowseFilter(BrowseFilterNone.copy(lossless = true, yearFrom = 1990, codec = "FLAC"))
        runCurrent()
        // Two changes in one breath are one read: the first is cancelled before it asks.
        assertEquals(2, engine.count("albums("))
        // The same value again is not a change.
        model.setAlbumSort(AlbumSort.TITLE)
        runCurrent()
        assertEquals(2, engine.count("albums("))

        val relaunched = model(FakeEngine(), store)
        assertEquals(AlbumSort.TITLE, relaunched.albumSort.value)
        assertEquals(BrowseFilterNone.copy(lossless = true, yearFrom = 1990, codec = "FLAC"), relaunched.browseFilter.value)
        assertEquals(3, relaunched.browseFilter.value.activeCount)
    }

    @Test
    fun `a browse filter off a browser does not read anything`() = runTest {
        val engine = FakeEngine()
        val model = model(engine)
        model.setBrowseFilter(BrowseFilterNone.copy(favourites = true))
        runCurrent()
        assertEquals(0, engine.count("albums(") + engine.count("artists(") + engine.count("trackListing("))
    }

    @Test
    fun `the track browser reads a page at a time and appends until it has them all`() = runTest {
        val engine = FakeEngine { name, args ->
            if (name == "trackListing") {
                val offset = args[4] as Int // an unsigned argument crosses a proxy as its underlying int
                val page = (offset until minOf(offset + 1000, 2500)).map { track(it.toLong()) }
                TrackListing(page, 2500uL)
            } else {
                null
            }
        }
        val model = model(engine)
        model.moveTo(Section.Tracks)
        runCurrent()
        assertEquals(2500L, model.trackTotal.value)
        assertEquals(2500, model.visibleTracks.value.size)
        assertEquals(3, engine.count("trackListing("))
        assertEquals((0L until 2500L).toList(), model.visibleTracks.value.map { it.id })
    }

    @Test
    fun `a reload cancels the pages of the listing it replaces`() = runTest {
        val engine = FakeEngine { name, args ->
            if (name == "trackListing") {
                val offset = args[4] as Int // an unsigned argument crosses a proxy as its underlying int
                TrackListing((offset until minOf(offset + 1000, 3000)).map { track(it.toLong()) }, 3000uL)
            } else {
                null
            }
        }
        val model = model(engine)
        model.moveTo(Section.Tracks)
        model.setTrackSort(TrackBrowseSort.TITLE)
        runCurrent()
        assertEquals(3000, model.visibleTracks.value.size)
        assertEquals((0L until 3000L).toList(), model.visibleTracks.value.map { it.id })
    }

    @Test
    fun `a shelf's heading opens the browser with the shelf's filter and its query`() = runTest {
        val engine = FakeEngine { name, _ -> if (name == "albums") emptyList<Any>() else null }
        val model = model(engine)
        model.moveTo(Section.Tracks)

        val section = model.browse(ShelfList.Albums, ShelfKind.Search("bowie"))
        assertEquals(Section.Albums, section)
        assertEquals(AlbumSort.BEST_MATCH, model.albumSort.value)
        assertEquals(TrackBrowseSort.BEST_MATCH, model.trackSort.value)
        val listing = model.prepare(section)!!
        assertEquals("bowie", listing.filter)
        model.show(listing)
        runCurrent()
        assertEquals("bowie", model.filter.value)
        // Spent by the move it made: the next section starts clean.
        val next = model.prepare(Section.Artists)!!
        assertEquals("", next.filter)

        model.browse(ShelfList.Albums, ShelfKind.Recent)
        assertTrue(model.browseFilter.value.recent)
        assertEquals(AlbumSort.LAST_PLAYED, model.albumSort.value)
    }

    @Test
    fun `the navigator's seam loads a section, a record and an artist`() = runTest {
        val engine = FakeEngine { name, _ ->
            when (name) {
                "albums" -> listOf(album(1))
                "albumPage" -> AlbumPage(album(5), listOf(track(1), track(2)))
                "artist" -> artist(3)
                else -> null
            }
        }
        val model = model(engine)
        val listing = model.prepare(Route.Page(Section.Albums))
        assertTrue(listing is Listing)
        model.arrive(Route.Page(Section.Albums), listing)
        assertEquals(Section.Albums, model.section.value)

        assertNull(model.prepare(Route.Album(5)))
        assertEquals(2, model.detailRecords.value.getValue(5).tracks.size)
        assertNull(model.prepare(Route.Artist(3)))
        runCurrent()
        assertEquals(artist(3), model.detailArtist.value?.artist)
        assertNull(model.prepare(Route.Library))
    }

    @Test
    fun `a record already read at this library version is not read again, and after a change it is`() = runTest {
        val engine = FakeEngine { name, _ -> if (name == "albumPage") AlbumPage(album(5), listOf(track(1))) else null }
        val model = model(engine)
        model.prepareAlbum(5)
        model.prepareAlbum(5)
        assertEquals(1, engine.count("albumPage("))
        mirror.apply(StateSlice.Library(1uL))
        model.prepareAlbum(5)
        assertEquals(2, engine.count("albumPage("))
        assertEquals(1L, model.detailRecords.value.getValue(5).stamp)
    }

    @Test
    fun `a failed read never replaces a good record`() = runTest {
        var failing = false
        val engine = FakeEngine { name, _ ->
            if (name != "albumPage") null else if (failing) throw KoanException.Audio("gone") else AlbumPage(album(5), listOf(track(1)))
        }
        val model = model(engine)
        model.prepareAlbum(5)
        mirror.apply(StateSlice.Library(1uL))
        failing = true
        model.prepareAlbum(5)
        val held = model.detailRecords.value.getValue(5)
        assertFalse(held.failed)
        assertEquals(0L, held.stamp)

        // A page never read does show the failure, as failed rather than empty.
        model.prepareAlbum(6)
        assertTrue(model.detailRecords.value.getValue(6).failed)
        assertNull(model.detailRecords.value.getValue(6).album)
    }

    @Test
    fun `only the most recent sixty-four records are held`() = runTest {
        val engine = FakeEngine { name, args -> if (name == "albumPage") AlbumPage(album(args[0] as Long), emptyList()) else null }
        val model = model(engine)
        for (id in 1L..66L) model.prepareAlbum(id)
        val held = model.detailRecords.value
        assertEquals(64, held.size)
        assertFalse(1L in held)
        assertFalse(2L in held)
        assertTrue(3L in held)
        assertTrue(66L in held)
    }

    @Test
    fun `an artist with no albums shows the tracks credited to them, and one with albums does not ask`() = runTest {
        var albums = emptyList<uniffi.koan_ffi.Album>()
        val engine = FakeEngine { name, _ ->
            when (name) {
                "albums" -> albums
                "tracks" -> listOf(track(9))
                "artist" -> artist(3)
                else -> null
            }
        }
        val model = model(engine)
        model.prepareArtist(3)
        assertEquals(listOf(track(9)), model.detailArtist.value?.appearances)
        assertEquals(1, engine.count("tracks("))
        albums = listOf(album(1))
        mirror.apply(StateSlice.Library(1uL))
        model.prepareArtist(3)
        assertEquals(1, engine.count("tracks("))
        assertEquals(emptyList<Any>(), model.detailArtist.value?.appearances)
    }

    @Test
    fun `a heart tap shows at once from the engine's answer`() = runTest {
        val engine = FakeEngine { name, _ -> if (name == "toggleFavourite") true else if (name == "toggleFavouriteAlbum") false else null }
        val model = model(engine)
        val seen = model.favouriteTrackIds.record(backgroundScope)
        model.toggleFavouriteTrack(4)
        model.toggleFavouriteAlbum(8)
        runCurrent()
        assertEquals(setOf(4L), model.favouriteTrackIds.value)
        assertEquals(emptySet<Long>(), model.favouriteAlbumIds.value)
        assertEquals(listOf(emptySet(), setOf(4L)), seen.toList())
    }

    @Test
    fun `refreshing favourites with the same ids wakes nobody`() = runTest {
        val engine = FakeEngine { name, _ ->
            when (name) {
                "favouriteTrackIds" -> listOf(1L, 2L)
                "favouriteAlbumIds", "favouriteArtistIds" -> emptyList<Long>()
                else -> null
            }
        }
        val model = model(engine)
        model.refreshFavourites(); runCurrent()
        val seen = model.favouriteTrackIds.record(backgroundScope)
        model.refreshFavourites(); runCurrent()
        assertEquals(1, seen.size)
        assertEquals(setOf(1L, 2L), model.favouriteTrackIds.value)
    }

    @Test
    fun `the favourites page reloads on a toggle, other pages do not`() = runTest {
        val engine = FakeEngine { name, _ ->
            when (name) {
                "toggleFavourite" -> true
                "shelfSummary" -> ShelfSummary(emptyList(), 0uL, emptyList(), 0uL, emptyList(), 0uL)
                else -> null
            }
        }
        val model = model(engine)
        model.moveTo(Section.Favourites)
        val before = engine.count("shelfSummary(")
        model.toggleFavouriteTrack(1); runCurrent()
        assertEquals(before + 1, engine.count("shelfSummary("))

        model.moveTo(Section.PlayHistory)
        val albums = engine.count("albums(")
        model.toggleFavouriteTrack(1); runCurrent()
        assertEquals(albums, engine.count("albums("))
    }

    @Test
    fun `history changes reload only what history derives`() = runTest {
        val engine = FakeEngine { name, _ -> if (name == "playHistory") emptyList<Any>() else null }
        val model = model(engine)
        model.moveTo(Section.Albums)
        model.historyChanged(); runCurrent()
        assertEquals(0, engine.count("playHistory("))
        model.moveTo(Section.PlayHistory)
        val after = engine.count("playHistory(")
        model.historyChanged(); runCurrent()
        assertEquals(after + 1, engine.count("playHistory("))
    }

    @Test
    fun `forgetting plays drops the rows locally before the engine answers`() = runTest {
        val rows = listOf(10L, 11L, 12L)
        val engine = FakeEngine { name, _ -> if (name == "playHistory") rows.map { playEntry(it) } else null }
        val model = model(engine)
        model.moveTo(Section.PlayHistory)
        model.forgetPlays(setOf(11L)); runCurrent()
        assertEquals(listOf(10L, 12L), model.visiblePlayHistory.value.map { it.id })
        assertTrue(engine.calls.any { it.startsWith("deletePlays(") })
    }

    @Test
    fun `a sync claims the remote rows, shows as a task, and is refused while one runs`() = runTest {
        val activity = ActivityModel()
        val engine = FakeEngine()
        val model = model(engine, activity = activity)
        model.syncRemote(); runCurrent()
        assertEquals(1, engine.count("syncRemote"))
        assertEquals("the row ends with the work", emptyList<ActivityTask>(), activity.tasks.value)

        val running = activity.begin("Syncing with server", setOf(Resource.RemoteTracks))
        model.syncRemote(); runCurrent()
        assertEquals("refused while one runs", 1, engine.count("syncRemote"))
        activity.end(running)
    }

    private fun playEntry(id: Long) = uniffi.koan_ffi.PlayHistoryEntry(
        id = id, track = track(id), playedAt = 0L, listenedMs = 1000L, source = "local",
    )
}
