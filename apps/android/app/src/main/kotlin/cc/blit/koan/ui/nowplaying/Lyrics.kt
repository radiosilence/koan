package cc.blit.koan.ui.nowplaying

import androidx.compose.animation.animateColorAsState
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.engine.Playhead
import cc.blit.koan.ffi.LyricLine
import cc.blit.koan.ffi.Lyrics
import cc.blit.koan.ui.koanApp
import kotlinx.coroutines.delay

/** What was found, and for which track, so words never show beside another song. */
private data class Loaded(val trackId: Long, val lyrics: Lyrics?)

/**
 * The words, in the sleeve's place: synced lines highlighted against the
 * playhead, else the plain text, else a note that there are none.
 *
 * The cache answers at once; LRCLIB is asked only on a miss or for a plain
 * copy that may have a synced one upstream by now.
 */
@Composable
fun LyricsPanel(trackId: Long?, durationMs: Long, modifier: Modifier = Modifier) {
    val mirror = koanApp().mirror
    var loaded by remember { mutableStateOf<Loaded?>(null) }
    var fetching by remember { mutableStateOf(false) }
    LaunchedEffect(trackId) {
        if (trackId == null) return@LaunchedEffect
        val engine = mirror.engine()
        val cached = runCatching { engine.lyrics(trackId) }.getOrNull()
        loaded = Loaded(trackId, cached)
        if (cached?.synced == true) return@LaunchedEffect
        fetching = true
        try {
            val fetched = runCatching { engine.fetchLyrics(trackId) }.getOrNull()
            loaded = Loaded(trackId, fetched ?: cached)
        } finally {
            fetching = false
        }
    }
    val lyrics = loaded?.takeIf { it.trackId == trackId }?.lyrics
    Box(modifier, contentAlignment = Alignment.Center) {
        when {
            lyrics != null && lyrics.lines.isNotEmpty() -> SyncedLyrics(lyrics.lines, durationMs)
            lyrics != null && lyrics.content.isNotBlank() -> Text(
                lyrics.content,
                Modifier.fillMaxSize().verticalScroll(rememberScrollState()),
                style = MaterialTheme.typography.bodyLarge,
            )
            fetching -> CircularProgressIndicator()
            else -> Text(
                if (trackId == null) "Nothing playing" else "No lyrics found",
                style = MaterialTheme.typography.titleMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

/** Last line whose time has passed. */
internal fun lineAt(lines: List<LyricLine>, positionSecs: Double) = lines.indexOfLast { it.timeSecs <= positionSecs }

@Composable
private fun SyncedLyrics(lines: List<LyricLine>, durationMs: Long) {
    val app = koanApp()
    val playhead by app.mirror.playhead.collectAsStateWithLifecycle()
    var active by remember { mutableStateOf(-1) }
    val list = rememberLazyListState()
    // A line changes when a timestamp passes, and every timestamp is known:
    // sleep until the next one rather than asking where the playhead is.
    // Restarted by the anchor, which moves on a seek, a pause or a new track.
    LaunchedEffect(lines, playhead) { follow(lines, playhead) { active = it } }
    LaunchedEffect(active) { if (active >= 0) list.animateScrollToItem(active, -list.layoutInfo.viewportSize.height / 3) }
    LazyColumn(
        Modifier.fillMaxSize(),
        state = list,
        contentPadding = PaddingValues(vertical = 120.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        itemsIndexed(lines) { index, line ->
            val colour by animateColorAsState(
                if (index == active) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.6f),
                label = "line",
            )
            Text(
                line.text.ifEmpty { "♪" },
                Modifier.fillMaxWidth().clickable(enabled = durationMs > 0) { app.player.seek((line.timeSecs * 1000).toLong()) },
                color = colour,
                style = MaterialTheme.typography.titleLarge,
                fontWeight = if (index == active) FontWeight.SemiBold else FontWeight.Normal,
            )
        }
    }
}

private suspend fun follow(lines: List<LyricLine>, playhead: Playhead, set: (Int) -> Unit) {
    while (true) {
        val position = playhead.positionAt() / 1000.0
        set(lineAt(lines, position))
        if (!playhead.playing) return
        val next = lines.firstOrNull { it.timeSecs > position } ?: return
        delay(((next.timeSecs - position) * 1000).toLong().coerceAtLeast(1))
    }
}
