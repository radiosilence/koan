package cc.blit.koan.ui.theme

import android.app.Application
import android.graphics.Bitmap
import androidx.compose.ui.graphics.asAndroidBitmap
import androidx.compose.ui.graphics.toPixelMap
import androidx.compose.ui.test.captureToImage
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onRoot
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import java.io.File

/**
 * The theme sheet on a phone-sized screen with a record's wash behind it,
 * written to `build/outputs/theme-sheet-wash.png` for review.
 */
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35], qualifiers = "w411dp-h891dp-xhdpi")
@GraphicsMode(GraphicsMode.Mode.NATIVE)
class ThemeSheetWashTest {
    @get:Rule
    val compose = createComposeRule()

    @Test
    fun theSheetSitsOnTheRecordsWash() {
        compose.mainClock.autoAdvance = false
        compose.setContent { ThemeSheet() }
        // The bake runs off the main thread; then the two-second dissolve.
        val deadline = System.currentTimeMillis() + 10_000
        while (System.currentTimeMillis() < deadline) {
            compose.mainClock.advanceTimeBy(100)
            Thread.sleep(20)
        }
        compose.mainClock.advanceTimeBy(4_000)
        val image = compose.onRoot().captureToImage()
        File("build/outputs").mkdirs()
        File("build/outputs/theme-sheet-wash.png").outputStream().use {
            image.asAndroidBitmap().compress(Bitmap.CompressFormat.PNG, 100, it)
        }
        // The ground is the record's colour, not bare `bg`: the default sleeve is red.
        val pixels = image.toPixelMap()
        val ground = pixels[pixels.width - 4, pixels.height / 2]
        assertTrue("ground is $ground", ground.red > ground.blue + 0.02f && ground.red > ground.green + 0.02f)
    }
}
