package cc.blit.koan.ui.nowplaying

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.draggable
import androidx.compose.foundation.gestures.rememberDraggableState
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.art.ArtSize
import cc.blit.koan.ffi.PlayState
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.theme.LocalKoanTokens
import coil3.compose.AsyncImage

/**
 * The transport, phone-sized, above the tabs while something is queued: the
 * sleeve, what is playing and how far through, play/pause and next. Tapped or
 * pulled up, it opens Now Playing.
 */
@Composable
fun MiniPlayer() {
    val app = koanApp()
    val nav = LocalNavigator.current
    val now by app.mirror.nowPlaying.collectAsStateWithLifecycle()
    val entry = now?.entry ?: return
    val playing = now?.state == PlayState.PLAYING
    val tokens = LocalKoanTokens.current
    val openAt = with(LocalDensity.current) { 48.dp.toPx() }
    var pulled by remember { mutableFloatStateOf(0f) }
    Surface(tonalElevation = 3.dp) {
        Column(
            Modifier
                .clickable(onClickLabel = "Open Now Playing") { nav.openNowPlaying() }
                .draggable(
                    orientation = Orientation.Vertical,
                    state = rememberDraggableState { dy -> pulled += dy },
                    onDragStarted = { pulled = 0f },
                    onDragStopped = { velocity -> if (pulled < -openAt || velocity < -1500f) nav.openNowPlaying() },
                ),
        ) {
            Progress(now?.durationMs?.toLong()?.takeIf { it > 0 } ?: entry.durationMs?.toLong() ?: 0)
            Row(
                Modifier.fillMaxWidth().height(tokens.miniPlayerHeight).padding(start = tokens.gutter, end = tokens.rowGap),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                val sleeve = Modifier.size(tokens.thumb).clip(RoundedCornerShape(6.dp))
                entry.art(ArtSize.Thumb)?.let { AsyncImage(it, null, sleeve) }
                    ?: Box(sleeve.background(MaterialTheme.colorScheme.surfaceVariant))
                Spacer(Modifier.size(tokens.rowGap * 1.5f))
                Column(Modifier.weight(1f)) {
                    Text(entry.title, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    if (entry.artist.isNotEmpty()) {
                        Text(
                            entry.artist,
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                }
                IconButton(onClick = app.player::togglePlayPause, Modifier.size(48.dp)) {
                    if (now?.waiting == true) {
                        CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                    } else {
                        KoanIcon(
                            if (playing) KoanIcons.pause else KoanIcons.play,
                            if (playing) "Pause" else "Play",
                        )
                    }
                }
                IconButton(onClick = app.player::next, Modifier.size(48.dp)) {
                    KoanIcon(KoanIcons.next, "Next")
                }
            }
        }
    }
}

/** How far through the track, along the bar's top edge. Its own scope, so the once-a-second tick redraws only this. */
@Composable
private fun Progress(durationMs: Long) {
    val playhead by koanApp().mirror.playhead.collectAsStateWithLifecycle()
    val position = rememberPosition(playhead)
    LinearProgressIndicator(
        progress = { if (durationMs > 0) (position.toFloat() / durationMs).coerceIn(0f, 1f) else 0f },
        modifier = Modifier.fillMaxWidth().height(2.dp),
        trackColor = MaterialTheme.colorScheme.surfaceVariant,
        gapSize = 0.dp,
        drawStopIndicator = {},
    )
}
