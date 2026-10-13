package cc.blit.koan.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.art.ArtSize
import cc.blit.koan.ui.nowplaying.art
import cc.blit.koan.ui.theme.KoanAccent
import cc.blit.koan.ui.theme.KoanTheme
import coil3.SingletonImageLoader
import coil3.request.ImageRequest
import coil3.request.allowHardware
import coil3.toBitmap
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * The theme as `[appearance]` sets it, in the accent of the record playing:
 * mint with nothing playing, a sleeve without a colour, or "colours from the
 * record" off. Until the engine has answered, the theme's defaults.
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
    val accent by produceState<KoanAccent?>(null, art) {
        value = art?.let {
            val request = ImageRequest.Builder(context).data(it).allowHardware(false).build()
            val image = SingletonImageLoader.get(context).execute(request).image ?: return@let null
            withContext(Dispatchers.Default) { KoanAccent.of(image.toBitmap()) }
        }
    }
    KoanTheme(
        koan = appearance?.koan != false,
        icons = appearance?.icons != false,
        accent = accent,
        content = content,
    )
}
