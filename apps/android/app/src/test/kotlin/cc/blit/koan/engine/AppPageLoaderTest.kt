package cc.blit.koan.engine

import cc.blit.koan.FakeEngine
import cc.blit.koan.album
import cc.blit.koan.art.ArtworkSource
import cc.blit.koan.art.ArtworkWarmer
import cc.blit.koan.nav.Navigator
import cc.blit.koan.nav.Route
import cc.blit.koan.nav.Section
import cc.blit.koan.nav.Tab
import cc.blit.koan.state.EngineMirror
import cc.blit.koan.state.KeyValueStore
import cc.blit.koan.state.LibraryModel
import cc.blit.koan.state.PlaylistsModel
import cc.blit.koan.track
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import uniffi.koan_ffi.AlbumPage
import uniffi.koan_ffi.PlaylistEntry

@OptIn(ExperimentalCoroutinesApi::class)
class AppPageLoaderTest {
    private val mirror = EngineMirror()

    @Test
    fun `the navigator moves onto a playlist only once its rows and the library are in hand`() = runTest {
        val warmed = mutableListOf<ArtworkSource>()
        val engine = FakeEngine { name, _ ->
            when (name) {
                "playlistTracks" -> listOf(PlaylistEntry(1, track(7)))
                "playlistCoverAlbumIds" -> listOf(3L, 4L)
                "albumPage" -> AlbumPage(album(3), emptyList())
                else -> null
            }
        }
        val library = LibraryModel(engine.engine, mirror, backgroundScope, KeyValueStore.Memory(), art = ArtworkWarmer { warmed += it })
        val playlists = PlaylistsModel(engine.engine, mirror, backgroundScope)
        val nav = Navigator(backgroundScope, AppPageLoader(library, playlists))

        nav.selectTab(Tab.Library); runCurrent()
        nav.openPlaylist(9); runCurrent()
        assertEquals(Route.playlist(9), nav.current)
        assertEquals(9L, playlists.openId.value)
        assertEquals(listOf(1L), playlists.entries.value.map { it.entryId })
        assertEquals("the room takes its colour from the first record", listOf<ArtworkSource>(ArtworkSource.Album(3)), warmed)
        // The library follows where you are: the playlist is its section now.
        assertEquals(Section.Playlist(9), library.section.value)

        nav.openAlbum(3); runCurrent()
        assertEquals(Route.Album(3), nav.current)
        assertEquals(3L, library.detailRecords.value.keys.single())
        assertNull(playlists.naming.value)
    }
}
