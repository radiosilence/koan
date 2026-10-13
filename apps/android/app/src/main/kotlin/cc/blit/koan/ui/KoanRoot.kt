package cc.blit.koan.ui

import android.graphics.Bitmap
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.art.Art
import cc.blit.koan.art.ArtSize
import cc.blit.koan.ffi.PlayState
import cc.blit.koan.ui.nowplaying.art
import cc.blit.koan.ui.theme.KoanAccent
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.ProvideRecordWash
import coil3.SingletonImageLoader
import coil3.request.ImageRequest
import coil3.request.allowHardware
import coil3.toBitmap
import kotlinx.coroutines.Dispatchers
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
    KoanTheme(
        koan = appearance?.koan != false,
        icons = appearance?.icons != false,
        accent = accent,
    ) {
        ProvideRecordWash(
            sleeve = sleeve.bitmap,
            pending = sleeve.art != art,
            playing = nowPlaying?.state == PlayState.PLAYING,
            recordColours = recordColours,
            content = content,
        )
    }
}
