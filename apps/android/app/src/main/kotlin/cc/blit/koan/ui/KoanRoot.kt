package cc.blit.koan.ui

import android.animation.ValueAnimator
import android.graphics.Bitmap
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.compose.currentStateAsState
import cc.blit.koan.art.Art
import cc.blit.koan.art.ArtSize
import cc.blit.koan.ffi.PlayState
import cc.blit.koan.ui.nowplaying.art
import cc.blit.koan.ui.theme.KoanAccent
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.LocalRainbowStep
import cc.blit.koan.ui.theme.ProvideRecordWash
import cc.blit.koan.ui.theme.Rainbow
import coil3.SingletonImageLoader
import coil3.request.ImageRequest
import coil3.request.allowHardware
import coil3.toBitmap
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext

/** A record's sleeve as answered: `art` is which cover it is for, `bitmap` null for one with none. */
private data class Sleeve(val art: Art?, val bitmap: Bitmap?)

/**
 * The theme as `[appearance]` sets it, in the colours of the record playing:
 * its accent, mint with nothing playing, a sleeve without a colour, or
 * "colours from the record" off; and its wash, for pages to draw. Until the
 * engine has answered, the theme's defaults.
 */
@Composable
fun KoanRoot(content: @Composable () -> Unit) {
    val app = koanApp()
    val appearance by app.mirror.appearance.collectAsStateWithLifecycle()
    val nowPlaying by app.mirror.nowPlaying.collectAsStateWithLifecycle()
    val graphics by app.graphics.level.collectAsStateWithLifecycle()
    val recordColours = appearance?.recordColours != false
    val art = nowPlaying?.entry?.art(ArtSize.Thumb)?.takeIf { recordColours }
    val context = LocalContext.current
    // Keyed on the cover, not the entry: the tracks of one record share it.
    // Held while the next is fetched, so the room keeps its colour meanwhile.
    val sleeve by produceState(Sleeve(null, null), art) {
        val bitmap = art?.let {
            val request = ImageRequest.Builder(context).data(it).allowHardware(false).build()
            SingletonImageLoader.get(context).execute(request).image?.toBitmap()
        }
        value = Sleeve(art, bitmap)
    }
    val accent by produceState<KoanAccent?>(null, sleeve) {
        value = sleeve.bitmap?.let { withContext(Dispatchers.Default) { KoanAccent.of(it) } }
    }
    val step = rainbowStep(appearance?.rainbow == true)
    KoanTheme(
        koan = appearance?.koan != false,
        icons = appearance?.icons != false,
        accent = step?.let(Rainbow::accent) ?: accent,
    ) {
        ProvideRecordWash(
            sleeve = sleeve.bitmap,
            pending = sleeve.art != art,
            playing = nowPlaying?.state == PlayState.PLAYING,
            recordColours = recordColours,
            graphics = graphics,
        ) {
            CompositionLocalProvider(LocalRainbowStep provides step, content = content)
        }
    }
}

/**
 * Where gay mode's accent is around the palette, or `null` with it off. It
 * moves every `Rainbow.PERIOD_MS` only while the app is in view and
 * animations are on; otherwise it holds its colour.
 */
@Composable
private fun rainbowStep(on: Boolean): Int? {
    if (!on) return null
    val lifecycle by LocalLifecycleOwner.current.lifecycle.currentStateAsState()
    val visible = lifecycle.isAtLeast(Lifecycle.State.STARTED)
    val animations = remember(visible) { ValueAnimator.areAnimatorsEnabled() }
    var step by remember { mutableIntStateOf(0) }
    LaunchedEffect(visible, animations) {
        while (visible && animations) {
            delay(Rainbow.PERIOD_MS)
            step++
        }
    }
    return step
}
