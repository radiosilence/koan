package cc.blit.koan.ui.library

import android.app.Application
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.grid.LazyGridState
import androidx.compose.material3.Text
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.unit.dp
import cc.blit.koan.ffi.Album
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/** The album grid over a library the size of a big one: lazy, and keyed so its place holds. */
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35], qualifiers = "w411dp-h891dp")
class AlbumGridTest {
    @get:Rule val compose = createComposeRule()

    private fun album(id: Long) = Album(id, "Album $id", 1, "Artist", emptyList(), null, null, null, null, null, null, null)

    private val library = (1L..10_000L).map(::album)

    @Test
    fun tenThousandRecordsComposeOnlyWhatIsOnScreen() {
        var live = 0
        var most = 0
        val state = LazyGridState()
        compose.setContent {
            AlbumGrid(library, state) { album ->
                DisposableEffect(album.id) {
                    live++
                    most = maxOf(most, live)
                    onDispose { live-- }
                }
                Text(album.title, Modifier.size(150.dp))
            }
        }
        compose.onNodeWithText("Album 1").assertExists()
        compose.runOnUiThread { runBlocking { state.scrollToItem(9_990) } }
        compose.waitForIdle()
        compose.onNodeWithText("Album 9991").assertExists()
        // Two columns of 150 dp at 411 dp wide: a screenful is a few dozen
        // tiles, prefetch included, never the library.
        assertTrue("composed at most $most tiles at once", most < 60)
    }

    @Test
    fun theRecordAtTheTopStaysThereWhenTheListingChangesAboveIt() {
        var albums by mutableStateOf(library)
        val state = LazyGridState()
        compose.setContent { AlbumGrid(albums, state) { Text(it.title, Modifier.size(150.dp)) } }
        compose.runOnUiThread { runBlocking { state.scrollToItem(5_000) } }
        compose.waitForIdle()
        val top = albums[state.firstVisibleItemIndex].id
        // A sync lands two records ahead of it, as a newest-first listing would.
        albums = listOf(album(20_001), album(20_002)) + library
        compose.waitForIdle()
        assertEquals(top, albums[state.firstVisibleItemIndex].id)
    }
}
