package cc.blit.koan.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.assertRangeInfoEquals
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.click
import androidx.compose.ui.test.down
import androidx.compose.ui.test.moveTo
import androidx.compose.ui.test.up
import androidx.compose.ui.semantics.ProgressBarRangeInfo
import androidx.compose.ui.unit.dp
import cc.blit.koan.state.PlayingLevels
import cc.blit.koan.theme.KoanSlider
import cc.blit.koan.theme.KoanTheme
import cc.blit.koan.ui.queue.PlayingBars
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import uniffi.koan_ffi.VizLevels
import uniffi.koan_ffi.VizStreamInterface

/**
 * The seek bar and the playing indicator, composed on the JVM by Robolectric.
 * This checks the gestures reach the callbacks and that the indicator listens
 * exactly while it is on screen; it says nothing of how either looks or feels,
 * which only a device can.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35], application = android.app.Application::class)
class QueueControlsTest {
    @get:Rule
    val compose = createComposeRule()

    @Test
    fun `a tap on the seek bar scrubs and commits there`() {
        val scrubbed = mutableListOf<Float>()
        val committed = mutableListOf<Float>()
        compose.setContent {
            KoanTheme { Box(Modifier.width(400.dp)) { KoanSlider({ 0.5f }, { scrubbed += it }, { committed += it }) } }
        }
        compose.onNodeWithContentDescription("Seek").performTouchInput { click(androidx.compose.ui.geometry.Offset(width * 0.25f, centerY)) }
        assertEquals(1, committed.size)
        assertEquals(0.25f, committed.single(), 0.01f)
        assertEquals(committed, scrubbed)
    }

    @Test
    fun `a drag scrubs along the way and commits once, where the finger lifted`() {
        val scrubbed = mutableListOf<Float>()
        val committed = mutableListOf<Float>()
        compose.setContent {
            KoanTheme { Box(Modifier.width(400.dp)) { KoanSlider({ 0f }, { scrubbed += it }, { committed += it }) } }
        }
        compose.onNodeWithContentDescription("Seek").performTouchInput {
            down(androidx.compose.ui.geometry.Offset(width * 0.1f, centerY))
            moveTo(androidx.compose.ui.geometry.Offset(width * 0.5f, centerY))
            moveTo(androidx.compose.ui.geometry.Offset(width * 0.8f, centerY))
            up()
        }
        assertTrue("scrubbed while dragging: $scrubbed", scrubbed.size >= 2)
        assertEquals(1, committed.size)
        assertEquals(0.8f, committed.single(), 0.02f)
    }

    @Test
    fun `a disabled seek bar takes no gestures`() {
        val committed = mutableListOf<Float>()
        compose.setContent {
            KoanTheme { Box(Modifier.width(400.dp)) { KoanSlider({ 0f }, {}, { committed += it }, enabled = false) } }
        }
        compose.onNodeWithContentDescription("Seek").performTouchInput { click() }
        assertTrue(committed.isEmpty())
    }

    @Test
    fun `the seek bar reports its progress to accessibility`() {
        compose.setContent { KoanTheme { Box(Modifier.fillMaxWidth()) { KoanSlider({ 0.4f }, {}, {}) } } }
        compose.onNodeWithContentDescription("Seek").assertRangeInfoEquals(ProgressBarRangeInfo(0.4f, 0f..1f))
    }

    private class Quiet : VizStreamInterface, AutoCloseable {
        var closed = false
        override suspend fun next(): VizLevels? = kotlinx.coroutines.awaitCancellation()
        override fun close() { closed = true }
    }

    @Test
    fun `the playing indicator listens while it is on screen and stops when it leaves`() {
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
        val streams = mutableListOf<Quiet>()
        val levels = PlayingLevels(scope, open = { Quiet().also { streams += it } }, setFps = {})
        var shown by mutableStateOf(true)
        var live by mutableStateOf(true)
        compose.setContent {
            KoanTheme { if (shown) PlayingBars(levels, live) }
        }
        compose.waitForIdle()
        assertEquals("listening", 1, levels.attachedCount)

        live = false
        compose.waitForIdle()
        assertEquals("paused: not listening", 0, levels.attachedCount)

        live = true
        compose.waitForIdle()
        assertEquals(1, levels.attachedCount)

        shown = false
        compose.waitForIdle()
        assertEquals("gone: not listening", 0, levels.attachedCount)
        assertTrue("and the stream is let go", streams.all { it.closed })
        scope.cancel()
    }
}
