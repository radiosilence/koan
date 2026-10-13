package cc.blit.koan.ui.theme

import android.app.Application
import android.graphics.Bitmap
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.SideEffect
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.graphics.toPixelMap
import androidx.compose.ui.test.captureToImage
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.unit.dp
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/**
 * What a frame of the drift costs the app: the wash's own draw, which reads
 * the drift, and nothing else. No recomposition, and the page over it is not
 * drawn again.
 */
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
@GraphicsMode(GraphicsMode.Mode.NATIVE)
class WashFrameTest {
    init {
        System.setProperty("robolectric.pixelCopyRenderMode", "hardware")
    }

    @get:Rule
    val compose = createComposeRule()

    @Test
    fun aFrameOfTheDriftRedrawsTheWashAlone() {
        // Two colours, so the drift moves something that can be seen.
        val sleeve = Bitmap.createBitmap(64, 64, Bitmap.Config.ARGB_8888).apply {
            for (x in 0 until 64) for (y in 0 until 64) setPixel(x, y, if (x < 32) 0xFFB0302A.toInt() else 0xFF2A50C0.toInt())
        }
        var wash: RecordWash? = null
        var compositions = 0
        var pageDraws = 0
        // The drift never ends, so the clock is stepped by hand: Compose is never idle while it runs.
        compose.mainClock.autoAdvance = false
        compose.setContent {
            KoanTheme(dark = true) {
                ProvideRecordWash(sleeve, pending = false, playing = true, recordColours = true) {
                    wash = LocalRecordWash.current
                    Box(Modifier.size(300.dp).recordWash()) {
                        SideEffect { compositions++ }
                        Box(Modifier.fillMaxSize().drawBehind { pageDraws++ })
                    }
                }
            }
        }
        val deadline = System.currentTimeMillis() + 10_000
        while (wash?.current == null && System.currentTimeMillis() < deadline) compose.mainClock.advanceTimeByFrame()
        // Past the dissolve, so what moves is the drift alone.
        compose.mainClock.advanceTimeBy(3_000)
        assertNotNull(wash?.current)
        // Drawn through the hardware renderer, which keeps each layer's
        // display list until something it read changes, as a device does.
        val first = compose.onRoot().captureToImage().toPixelMap()
        val composed = compositions
        val drawn = pageDraws
        val before = wash!!.scale.value to wash!!.travel.value

        repeat(60) { compose.mainClock.advanceTimeByFrame() }
        val after = compose.onRoot().captureToImage()

        assertNotEquals("the drift moved", before, wash!!.scale.value to wash!!.travel.value)
        assertEquals("no recomposition", composed, compositions)
        assertTrue("the page was drawn at all", drawn > 0)
        assertEquals("the page was not drawn again", drawn, pageDraws)
        val moved = after.toPixelMap()
        val changed = (0 until moved.width step 7).any { x -> (0 until moved.height step 7).any { y -> moved[x, y] != first[x, y] } }
        assertTrue("the wash was drawn again, moved", changed)
    }
}
