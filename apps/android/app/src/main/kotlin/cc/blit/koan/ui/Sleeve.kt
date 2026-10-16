package cc.blit.koan.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Dp
import cc.blit.koan.art.ArtImageLoader
import cc.blit.koan.art.ArtworkSource
import cc.blit.koan.theme.KoanTheme
import coil3.compose.AsyncImage

/**
 * A sleeve: square, on the ground's `surface` until the bitmap arrives, and
 * left bare if the record has none. The request is built from the draw size,
 * so a row and a tile each hold a bitmap their own size (see
 * [ArtImageLoader.request]); nothing here reaches the engine, which is the
 * fetcher's business on the IO dispatcher.
 */
@Composable
fun Sleeve(
    source: ArtworkSource,
    size: Dp,
    modifier: Modifier = Modifier,
    description: String? = null,
) {
    val context = LocalContext.current
    val px = with(LocalDensity.current) { size.roundToPx() }
    val request = remember(source, px) { ArtImageLoader.request(context, source, px) }
    AsyncImage(
        model = request,
        contentDescription = description,
        modifier = modifier.size(size).background(KoanTheme.colors.surface),
        contentScale = ContentScale.Crop,
    )
}
