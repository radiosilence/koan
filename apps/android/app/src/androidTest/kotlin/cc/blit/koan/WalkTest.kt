package cc.blit.koan

import android.content.Intent
import android.view.accessibility.AccessibilityWindowInfo
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.By
import androidx.test.uiautomator.BySelector
import androidx.test.uiautomator.Direction
import androidx.test.uiautomator.StaleObjectException
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.UiObject2
import androidx.test.uiautomator.Until
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.regex.Pattern

/**
 * Walks the app page by page and keeps a screenshot of each, as the iPhone's
 * `WalkTests` does, in the same order and under the same names.
 *
 * Not an assertion suite: what is worth checking about the pages is how they
 * look. `just android-walk` signs the app in to a throwaway server, sets the
 * look and the appearance, runs this once for each, and pulls the screenshots.
 * A page that cannot be reached is skipped rather than failing the walk.
 */
@RunWith(AndroidJUnit4::class)
class WalkTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val device = UiDevice.getInstance(instrumentation)
    private val args = InstrumentationRegistry.getArguments()
    private val out = File(instrumentation.targetContext.getExternalFilesDir(null), "walk")

    @Test
    fun walk() {
        out.deleteRecursively()
        out.mkdirs()
        val context = instrumentation.targetContext
        context.startActivity(
            Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK),
        )
        device.wait(Until.hasObject(By.pkg(context.packageName).depth(0)), 10_000)
        pause(3)
        // The launch sync, on the first run against a server.
        pause(args.getString("settle")?.toDoubleOrNull() ?: 0.0)

        tab("Queue")
        snap("01-queue")
        tab("Library")
        snap("02-library")
        if (open(label("Albums"))) {
            pause(3)
            snap("03-albums")
            // The first sleeve: above the topmost, leftmost of the library's
            // record titles, since the title's line also links the artist.
            val title = device.wait(Until.findObjects(By.text(Pattern.compile("(?i)album \\d+|record \\d+|thirty rooms"))), 3_000)
                ?.minWithOrNull(compareBy({ it.visibleBounds.top }, { it.visibleBounds.left }))
                ?.visibleBounds
            if (title != null) {
                device.click(title.centerX(), title.top - title.height() * 3)
                pause(3)
                snap("04-album")
                // What the queue, the mini player and Now Playing show from here on.
                tap { find(label("Play")) }
                pause(2)
                back()
            }
            back()
        }
        if (open(label("Artists"))) {
            pause(3)
            snap("05-artists")
            back()
        }
        for ((name, shot) in listOf(
            "Favourites" to "05b-favourites", "Playlists" to "05c-playlists",
            "Recently Played" to "05d-recently-played", "Downloaded" to "05g-downloaded",
            "History" to "05e-history", "Downloads" to "05f-downloads",
        )) {
            if (!open(label(name))) continue
            pause(3)
            snap(shot)
            if (name == "Playlists" && open(By.text(Pattern.compile("Playlist \\d+")))) {
                pause(2)
                snap("05c2-playlist")
                back()
            }
            back()
        }

        // Now Playing, from the mini player over the tab bar: its title, left
        // of its Next button.
        find(By.desc("Next"))?.let { next ->
            device.click(device.displayWidth * 35 / 100, next.visibleCenter.y)
            if (find(By.desc("Show lyrics"), 5_000) != null) {
                pause(2)
                snap("06-now-playing")
                tap { find(By.desc("Show lyrics")) }
                pause(4)
                snap("07-lyrics")
                tap { find(By.desc("Show artwork")) }
                pause(1)
                if (!tap { find(By.desc("Close")) }) back()
                pause(1)
            }
        }

        tab("Settings")
        pause(1)
        snap("09-settings")
        // In the order Settings lists them, so each is found scrolling down from the last.
        for ((name, shot) in listOf(
            "Server" to "09b-server-end", "Account" to "09d-account", "People" to "09e-people",
            "Playback" to "09j-playback", "EQ" to "09h-eq", "Downloads" to "09k-downloads",
            "Devices" to "09f-devices", "Integrations" to "09g-server-capabilities",
            "Appearance" to "09c-appearance", "About" to "09l-about",
        )) {
            if (!open(label(name))) continue
            pause(if (name == "EQ") 2 else 1)
            when (name) {
                "Server" -> {
                    repeat(2) { scrollable()?.scroll(Direction.DOWN, 1f) }
                    pause(1)
                }
                "EQ" -> {
                    snap(shot)
                    scrollable()?.scroll(Direction.DOWN, 0.6f)
                    pause(1)
                }
            }
            snap(if (name == "EQ") "09i-eq-more" else shot)
            back()
        }

        tab("Queue")
        pause(1)
        snap("10-queue-playing")

        // Last, as on the iPhone, because its keyboard covers the tab bar.
        tab("Search")
        val term = args.getString("search") ?: "Long"
        find(By.clazz("android.widget.EditText"), 8_000)?.let { field ->
            field.click()
            field.text = term
            pause(3)
            // The results are what is worth seeing, not the keyboard over them.
            if (keyboardShown()) device.pressBack()
            pause(2)
        }
        snap("08-search")
        // The artist the search found, as its pill is tapped.
        if (open(By.clazz("android.widget.TextView").text(Pattern.compile("(?i)${Pattern.quote(term)}.*")))) {
            pause(4)
            snap("08a-artist")
        }
    }

    /** Text the app writes, which the kōan theme lowercases. */
    private fun label(text: String): BySelector = By.text(Pattern.compile("(?i)${Pattern.quote(text)}"))

    /** A tab: the lowest of the matches, since a page can carry its tab's name as its title. */
    private fun tab(name: String) {
        tap { device.wait(Until.findObjects(label(name)), 3_000)?.maxByOrNull { it.visibleBounds.top } }
        pause(1)
    }

    /** What matches, scrolled to, either way, if the page has to be moved to show it. */
    private fun find(selector: BySelector, timeout: Long = 3_000): UiObject2? =
        device.wait(Until.findObject(selector), timeout)
            ?: scrollable()?.scrollUntil(Direction.DOWN, Until.findObject(selector))
            ?: scrollable()?.scrollUntil(Direction.UP, Until.findObject(selector))

    private fun scrollable() = device.findObjects(By.scrollable(true)).maxByOrNull { it.visibleBounds.height() }

    private fun open(selector: BySelector) = tap { find(selector) }

    /** Taps what `locate` finds, finding it again if the page recomposed under it. */
    private fun tap(locate: () -> UiObject2?): Boolean {
        repeat(3) {
            val found = locate() ?: return false
            try {
                found.click()
                return true
            } catch (_: StaleObjectException) {
                pause(1)
            }
        }
        return false
    }

    private fun back() {
        device.pressBack()
        pause(1)
    }

    private fun keyboardShown() =
        instrumentation.uiAutomation.windows.any { it.type == AccessibilityWindowInfo.TYPE_INPUT_METHOD }

    private fun pause(seconds: Double) = Thread.sleep((seconds * 1000).toLong())

    private fun pause(seconds: Int) = pause(seconds.toDouble())

    private fun snap(name: String) {
        device.waitForIdle(2_000)
        device.takeScreenshot(File(out, "$name.png"))
    }
}
