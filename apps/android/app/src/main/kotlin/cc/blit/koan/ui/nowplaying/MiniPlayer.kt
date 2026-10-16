package cc.blit.koan.ui.nowplaying

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.draggable
import androidx.compose.foundation.gestures.rememberDraggableState
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.art.ArtSize
import cc.blit.koan.ffi.PlayState
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.library.PlayingBars
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.settings.PlayOnButton
import cc.blit.koan.ui.settings.Reach
import cc.blit.koan.ui.settings.controlled
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanEdge
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import cc.blit.koan.ui.theme.LocalKoanTokens
import cc.blit.koan.ui.theme.koanRule
import cc.blit.koan.ui.theme.koanSurface
import coil3.compose.AsyncImage

/**
 * The transport, phone-sized, always above the tabs: how far through as an
 * accent line along its top, the sleeve, what is playing beside the music's
 * level, Play on while there is another device, play/pause and next. Tapped or
 * pulled up, it opens Now Playing. With nothing queued it says so and keeps
 * Play on, so a phone works as a remote from a cold start, as iOS's does.
 */
@Composable
fun MiniPlayer() {
    val app = koanApp()
    val nav = LocalNavigator.current
    val now by app.mirror.nowPlaying.collectAsStateWithLifecycle()
    val devices by app.mirror.devices.collectAsStateWithLifecycle()
    val entry = now?.entry
    val playing = now?.state == PlayState.PLAYING
    val tokens = LocalKoanTokens.current
    val openAt = with(LocalDensity.current) { 48.dp.toPx() }
    var pulled by remember { mutableFloatStateOf(0f) }
    Column(
        Modifier
            .koanSurface()
            .koanRule(KoanEdge.Top)
            .clickable(remember { MutableInteractionSource() }, indication = null, enabled = entry != null, onClickLabel = "Open Now Playing") { nav.openNowPlaying() }
            .draggable(
                orientation = Orientation.Vertical,
                enabled = entry != null,
                state = rememberDraggableState { dy -> pulled += dy },
                onDragStarted = { pulled = 0f },
                onDragStopped = { velocity -> if (pulled < -openAt || velocity < -1500f) nav.openNowPlaying() },
            ),
    ) {
        Progress(now.length())
        Row(
            Modifier.fillMaxWidth().height(tokens.miniPlayerHeight).padding(start = KoanSpace.l, end = KoanSpace.xs),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            val sleeve = Modifier.size(tokens.thumb).clip(if (KoanTheme.isOn) RectangleShape else RoundedCornerShape(6.dp))
            entry?.art(ArtSize.Thumb)?.let { AsyncImage(it, null, sleeve) }
                ?: Box(sleeve.background(KoanTheme.color(KoanTone.Rule)))
            Spacer(Modifier.width(KoanSpace.m))
            Column(Modifier.weight(1f)) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(KoanSpace.s)) {
                    PlayingBars(playing, size = 10.dp, color = KoanTheme.color(if (playing) KoanTone.Accent else KoanTone.Muted))
                    KoanText(entry?.title ?: KoanTheme.label("Nothing playing"), role = KoanType.Meta, tone = if (entry == null) KoanTone.Muted else KoanTone.Strong, maxLines = 1)
                }
                // Where it is playing matters more than who by, when it is not here.
                val c = controlled(devices)
                when {
                    c != null -> Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(KoanSpace.xs)) {
                        val tone = if (c.reach is Reach.OutOfReach) KoanTone.Bad else KoanTone.Accent
                        KoanIcon(KoanIcons.devices, contentDescription = null, size = 12.sp, color = KoanTheme.color(tone))
                        KoanText(c.headline, role = KoanType.Fine, tone = tone, maxLines = 1)
                    }
                    !entry?.artist.isNullOrEmpty() -> KoanText(entry!!.artist, role = KoanType.Fine, tone = KoanTone.Muted, maxLines = 1)
                }
            }
            // Reachable with nothing playing here, which is when a phone is
            // most likely to be wanted as a remote.
            if (devices.target != null || devices.devices.isNotEmpty()) PlayOnButton(labelled = false)
            KoanButton(app.player::togglePlayPause, kind = KoanButtonKind.Icon, enabled = entry != null) {
                if (now?.waiting == true) {
                    CircularProgressIndicator(Modifier.size(18.dp), color = KoanTheme.color(KoanTone.Ink), strokeWidth = 2.dp)
                } else {
                    KoanIcon(if (playing) KoanIcons.pause else KoanIcons.play, if (playing) "Pause" else "Play", size = 18.sp)
                }
            }
            KoanButton(app.player::next, kind = KoanButtonKind.Icon, enabled = entry != null) {
                KoanIcon(KoanIcons.next, "Next", size = 16.sp)
            }
        }
    }
}

/** The playhead as a 2-dp accent line. Its own scope, so the once-a-second tick redraws only this. */
@Composable
private fun Progress(durationMs: Long) {
    val playhead by koanApp().mirror.playhead.collectAsStateWithLifecycle()
    val position = rememberPosition(playhead, durationMs)
    val fraction = if (durationMs > 0) (position.toFloat() / durationMs).coerceIn(0f, 1f) else 0f
    Box(Modifier.fillMaxWidth().height(2.dp)) {
        Box(Modifier.fillMaxWidth(fraction).height(2.dp).background(KoanTheme.look.accent))
    }
}
