package cc.blit.koan.ui.library

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.widthIn
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import cc.blit.koan.ui.theme.koanPill

/**
 * An artist among records and tracks, going to the artist: the artist glyph,
 * or a tick while picking, and the name, on the theme's pill.
 */
@Composable
fun ArtistPill(id: Long, name: String, selection: Selection<Playable>) {
    val nav = LocalNavigator.current
    val playable = Playable.OfArtist(id, name)
    var menu by remember { mutableStateOf(false) }
    Box {
        Row(
            Modifier
                .widthIn(max = 260.dp)
                .pickable(playable, selection, { menu = true }) { nav.push(ArtistRoute(id)) }
                .koanPill(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(KoanSpace.xs),
        ) {
            if (selection.active) {
                Tick(selection.contains(playable))
            } else {
                KoanIcon(KoanIcons.artist, null, size = KoanType.Fine.koan.fontSize, color = KoanTheme.color(KoanTone.Muted))
            }
            KoanText(name, role = KoanType.Control, maxLines = 1)
        }
        PlayableMenu(playable, menu) { menu = false }
    }
}
