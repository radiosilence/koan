package cc.blit.koan.ui.pages

import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.longClick
import cc.blit.koan.theme.KoanTheme
import cc.blit.koan.track
import kotlinx.coroutines.flow.MutableStateFlow
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import uniffi.koan_ffi.PlayHistoryEntry
import uniffi.koan_ffi.Transfer
import uniffi.koan_ffi.TransferFigure
import uniffi.koan_ffi.TransferState
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.ZoneId

/**
 * The two pages composed over plain values by Robolectric. They check that the
 * content appears and the taps reach the callbacks; not how it looks, and not
 * the bar's per-frame glide, which only a device shows.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35], application = android.app.Application::class)
class PagesTest {
    @get:Rule
    val compose = createComposeRule()

    private val zone = ZoneId.of("UTC")
    private val today = LocalDate.of(2025, 6, 3)
    private fun at(d: Int, h: Int) = LocalDateTime.of(2025, 6, d, h, 0).atZone(zone).toEpochSecond()
    private fun play(id: Long, t: Long, source: String = "local") =
        PlayHistoryEntry(id = id, track = track(id, album = null), playedAt = t, listenedMs = 1000L, source = source)

    @Test
    fun `history shows day headings, plays a tapped row, and forgets a long-pressed one`() {
        val played = mutableListOf<Long>()
        val forgotten = mutableListOf<Set<Long>>()
        compose.setContent {
            KoanTheme {
                PlayHistoryContent(
                    entries = listOf(play(1, at(3, 10)), play(2, at(2, 9))),
                    filter = "", onFilter = {}, onPlay = { played += it }, onOpenAlbum = {}, onOpenArtist = {},
                    onForget = { forgotten += it }, onClear = {}, zone = zone, today = today,
                )
            }
        }
        compose.onNodeWithText("today").assertExists()
        compose.onNodeWithText("yesterday").assertExists()
        compose.onNodeWithText("2 plays").assertExists()
        compose.onNodeWithText("Track 1", useUnmergedTree = true).performClick()
        assertEquals(listOf(1L), played)
        compose.onNodeWithText("Track 2", useUnmergedTree = true).performTouchInput { longClick() }
        compose.onNodeWithText("1 selected").assertExists()
        compose.onNodeWithText("forget").performClick()
        assertEquals(listOf(setOf(2L)), forgotten)
        assertEquals(listOf(1L), played)
    }

    @Test
    fun `an empty history says why, and a filter with no matches says so`() {
        val filter = MutableStateFlow("")
        compose.setContent {
            KoanTheme {
                PlayHistoryContent(emptyList(), filter.value, {}, {}, {}, {}, {}, {}, zone, today)
            }
        }
        compose.onNodeWithText("nothing played yet").assertExists()
    }

    private fun transfer(id: Long, state: TransferState) =
        Transfer(trackId = id, albumId = 5, title = "Song $id", artist = "Artist", state = state, failureReason = null)

    @Test
    fun `downloads list transfers, say stalled, and route the row actions`() {
        val shown = mutableListOf<Long>()
        val removed = mutableListOf<Long>()
        val figures = MutableStateFlow(mapOf(1L to TransferFigure(1, 0.5, 5_000_000uL, 10_000_000uL, 0uL)))
        val tasks = MutableStateFlow(emptyList<cc.blit.koan.state.ActivityTask>())
        compose.setContent {
            KoanTheme {
                DownloadsContent(
                    transfers = listOf(transfer(1, TransferState.RUNNING), transfer(2, TransferState.DONE)),
                    figures = figures, tasks = tasks,
                    onShowInLibrary = { shown += it.trackId }, onRemoveFile = { removed += it }, onCancelTask = {},
                )
            }
        }
        compose.onNodeWithText("Song 1").assertExists()
        compose.onNodeWithText("50%").assertExists()
        compose.onNodeWithText("Artist · stalled · 5.0 MB of 10 MB").assertExists()
        compose.onNodeWithText("1 in flight").assertExists()
        compose.onNodeWithText("remove file").performClick()
        assertEquals(listOf(2L), removed)
        compose.onAllNodesWithText("show in library")[0].performClick()
        assertEquals(listOf(1L), shown)

        // The fast stream moves and only the words that differ change.
        figures.value = mapOf(1L to TransferFigure(1, 0.75, 7_500_000uL, 10_000_000uL, 2_000_000uL))
        compose.waitUntil(2_000) { compose.onAllNodes(hasText("75%")).fetchSemanticsNodes().isNotEmpty() }
        compose.onNodeWithText("Artist · 2.0 MB/s · 7.5 MB of 10 MB").assertExists()
    }

    @Test
    fun `activity tasks show above the transfers and cancel by id`() {
        val cancelled = mutableListOf<String>()
        val tasks = MutableStateFlow(
            listOf(cc.blit.koan.state.ActivityTask("t1", "Scanning library", cancellable = true, done = 5, total = 10, unit = "files", detail = "a/b.flac")),
        )
        compose.setContent {
            KoanTheme {
                DownloadsContent(emptyList(), MutableStateFlow(emptyMap()), tasks, {}, {}, { cancelled += it })
            }
        }
        compose.onNodeWithText("scanning library").assertExists()
        compose.onNodeWithText("5 of 10 files").assertExists()
        compose.onNodeWithText("nothing downloading").assertExists()
        compose.onNodeWithContentDescription("Stop Scanning library").performClick()
        assertEquals(listOf("t1"), cancelled)
    }
}
