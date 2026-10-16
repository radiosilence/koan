package cc.blit.koan.ui.playlists

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.height
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.Dp
import cc.blit.koan.art.ArtworkSource
import cc.blit.koan.theme.KoanIcon
import cc.blit.koan.theme.KoanTheme
import cc.blit.koan.theme.KoanTone
import cc.blit.koan.ui.Sleeve

/**
 * A playlist's face: up to four records on a square. The port of
 * `PlaylistArtwork`: nothing is the playlist glyph on `surface`, one fills it,
 * two sit side by side (sleeves are square, so stacked halves would crop both
 * to letterboxes), three put the third across the bottom so no quarter is
 * empty, four are the 2 by 2.
 */
@Composable
fun PlaylistMosaic(sources: List<ArtworkSource>, size: Dp, modifier: Modifier = Modifier) {
    val half = size / 2
    Box(modifier.size(size).background(KoanTheme.colors.surface)) {
        when (sources.size) {
            0 -> Box(Modifier.size(size), contentAlignment = androidx.compose.ui.Alignment.Center) {
                KoanIcon("playlist", tone = KoanTone.Muted, textSizeDp = (size.value / 2.4f).coerceAtLeast(12f))
            }
            1 -> Sleeve(sources[0], size)
            2 -> Row {
                Sleeve(sources[0], size, Modifier.width(half).height(size))
                Sleeve(sources[1], size, Modifier.width(half).height(size))
            }
            3 -> Column {
                Row {
                    Sleeve(sources[0], half)
                    Sleeve(sources[1], half)
                }
                Sleeve(sources[2], size, Modifier.width(size).height(half))
            }
            else -> Column {
                Row {
                    Sleeve(sources[0], half)
                    Sleeve(sources[1], half)
                }
                Row {
                    Sleeve(sources[2], half)
                    Sleeve(sources[3], half)
                }
            }
        }
    }
}
