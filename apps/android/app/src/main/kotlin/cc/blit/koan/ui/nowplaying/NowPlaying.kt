package cc.blit.koan.ui.nowplaying

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.R
import cc.blit.koan.art.Art
import cc.blit.koan.art.ArtSize
import cc.blit.koan.engine.Playhead
import cc.blit.koan.ffi.PlayState
import cc.blit.koan.ffi.QueueItem
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.theme.LocalKoanTokens
import coil3.compose.AsyncImage
import kotlinx.coroutines.delay
import kotlinx.serialization.Serializable

/** What is playing, over the tabs. */
@Serializable
data object NowPlayingRoute : NavKey

fun EntryProviderScope<NavKey>.nowPlayingEntries() {
    entry<NowPlayingRoute> { NowPlayingPage() }
}

/**
 * The playhead's position, moving while it plays: the engine sends an anchor
 * when the playhead stops being where it would be, and this counts on from it,
 * waking once a second, on the second, to redraw the clock.
 */
@Composable
fun rememberPosition(playhead: Playhead): Long {
    var position by remember { mutableLongStateOf(playhead.positionAt()) }
    LaunchedEffect(playhead) {
        position = playhead.positionAt()
        while (playhead.playing) {
            delay(1000 - position % 1000)
            position = playhead.positionAt()
        }
    }
    return position
}

/** A queue row's cover: its record's, which lists already hold, else its own. */
fun QueueItem.art(size: ArtSize) = albumId?.let { Art.album(it, size) } ?: trackId?.let { Art.track(it, size) }

fun clock(ms: Long): String {
    val s = ms.coerceAtLeast(0) / 1000
    return "%d:%02d".format(s / 60, s % 60)
}

@Composable
private fun NowPlayingPage() {
    val app = koanApp()
    val nav = LocalNavigator.current
    val now by app.mirror.nowPlaying.collectAsStateWithLifecycle()
    val playhead by app.mirror.playhead.collectAsStateWithLifecycle()
    val position = rememberPosition(playhead)
    val entry = now?.entry
    BackHandler { nav.closeNowPlaying() }
    Surface(Modifier.fillMaxSize()) {
        Column(
            Modifier.safeDrawingPadding().padding(LocalKoanTokens.current.gutter),
            verticalArrangement = Arrangement.spacedBy(16.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            entry?.art(ArtSize.Full)?.let { AsyncImage(it, null, Modifier.fillMaxWidth().aspectRatio(1f)) }
            Text(entry?.title ?: "Nothing playing", style = MaterialTheme.typography.titleLarge)
            entry?.let { Text("${it.artist} · ${it.album}") }
            Text("${clock(position)} / ${clock(now?.durationMs?.toLong() ?: 0)}")
            Transport(now?.state == PlayState.PLAYING)
        }
    }
}

@Composable
private fun Transport(playing: Boolean) {
    val player = koanApp().player
    Row(verticalAlignment = Alignment.CenterVertically) {
        IconButton(onClick = player::previous) { Icon(painterResource(R.drawable.ic_previous), "Previous") }
        IconButton(onClick = player::togglePlayPause) {
            Icon(
                painterResource(if (playing) R.drawable.ic_pause else R.drawable.ic_play),
                if (playing) "Pause" else "Play",
            )
        }
        IconButton(onClick = player::next) { Icon(painterResource(R.drawable.ic_next), "Next") }
    }
}

/** The bar above the tabs while something is queued; opens Now Playing. */
@Composable
fun MiniPlayer() {
    val app = koanApp()
    val nav = LocalNavigator.current
    val now by app.mirror.nowPlaying.collectAsStateWithLifecycle()
    val entry = now?.entry ?: return
    val playing = now?.state == PlayState.PLAYING
    val tokens = LocalKoanTokens.current
    Surface(tonalElevation = 3.dp) {
        Column {
            HorizontalDivider()
            Row(
                Modifier.fillMaxWidth().height(tokens.miniPlayerHeight).clickable { nav.openNowPlaying() }
                    .padding(horizontal = tokens.gutter),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                entry.art(ArtSize.Thumb)?.let { AsyncImage(it, null, Modifier.size(tokens.thumb)) }
                Spacer(Modifier.size(tokens.rowGap))
                Column(Modifier.weight(1f)) {
                    Text(entry.title, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(entry.artist, style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                IconButton(onClick = app.player::togglePlayPause) {
                    Icon(
                        painterResource(if (playing) R.drawable.ic_pause else R.drawable.ic_play),
                        if (playing) "Pause" else "Play",
                    )
                }
                IconButton(onClick = app.player::next) { Icon(painterResource(R.drawable.ic_next), "Next") }
            }
        }
    }
}
