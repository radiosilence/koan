package cc.blit.koan.ui.nowplaying

import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.engine.Playhead
import cc.blit.koan.ffi.KoanException
import cc.blit.koan.ffi.LyricLine
import cc.blit.koan.ffi.Lyrics
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.theme.KoanDivider
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import cc.blit.koan.ui.theme.KoanUnavailable
import kotlinx.coroutines.delay

/** What was found, and for which track, so words never show beside another song. */
private data class Loaded(val trackId: Long, val lyrics: Lyrics?)

/**
 * The words, in the sleeve's place: synced lines highlighted against the
 * playhead, else the plain text, else a note that there are none. The header
 * names where they came from.
 *
 * The cache answers at once; LRCLIB is asked only on a miss or for a plain
 * copy that may have a synced one upstream by now.
 */
@Composable
fun LyricsPanel(trackId: Long?, durationMs: Long, modifier: Modifier = Modifier) {
    val mirror = koanApp().mirror
    var loaded by remember { mutableStateOf<Loaded?>(null) }
    // The track LRCLIB is being asked about: a cancelled ask for the last one
    // must not clear the next one's.
    var fetching by remember { mutableStateOf<Long?>(null) }
    LaunchedEffect(trackId) {
        if (trackId == null) return@LaunchedEffect
        val engine = mirror.engine()
        // A failed lookup is no lyrics; a cancelled one, for a track gone by, writes nothing.
        val cached = try {
            engine.lyrics(trackId)
        } catch (e: KoanException) {
            null
        }
        loaded = Loaded(trackId, cached)
        if (cached?.synced == true) return@LaunchedEffect
        fetching = trackId
        try {
            val fetched = try {
                engine.fetchLyrics(trackId)
            } catch (e: KoanException) {
                null
            }
            loaded = Loaded(trackId, fetched ?: cached)
        } finally {
            if (fetching == trackId) fetching = null
        }
    }
    // Nothing is said about a track until the cache has answered for it.
    val answered = loaded?.trackId == trackId
    // Only words worth showing: an empty copy is "No lyrics found", uncredited.
    val lyrics = loaded?.takeIf { answered }?.lyrics?.takeIf { it.lines.isNotEmpty() || it.content.isNotBlank() }
    val loading = trackId != null && (fetching == trackId || !answered)
    Column(modifier) {
        Row(Modifier.fillMaxWidth().padding(vertical = KoanSpace.m), verticalAlignment = Alignment.CenterVertically) {
            KoanText("Lyrics", Modifier.weight(1f).semantics { heading() }, appText = true)
            when {
                loading -> CircularProgressIndicator(Modifier.size(14.dp), color = KoanTheme.color(KoanTone.Muted), strokeWidth = 2.dp)
                else -> lyrics?.source?.let { KoanText(it, role = KoanType.Fine, tone = KoanTone.Muted) }
            }
        }
        KoanDivider()
        Box(Modifier.fillMaxWidth().weight(1f), contentAlignment = Alignment.Center) {
            when {
                lyrics != null && lyrics.lines.isNotEmpty() -> SyncedLyrics(lyrics.lines, durationMs)
                lyrics != null ->
                    KoanText(
                        lyrics.content,
                        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(vertical = KoanSpace.l),
                        role = KoanType.Control,
                    )
                loading -> Unit
                else -> KoanUnavailable(if (trackId == null) "Nothing playing" else "No lyrics found", KoanIcons.lyricsText, null)
            }
        }
    }
}

/** Last line whose time has passed. */
internal fun lineAt(lines: List<LyricLine>, positionSecs: Double) = lines.indexOfLast { it.timeSecs <= positionSecs }

@Composable
private fun SyncedLyrics(lines: List<LyricLine>, durationMs: Long) {
    val app = koanApp()
    val playhead by app.mirror.playhead.collectAsStateWithLifecycle()
    var active by remember { mutableIntStateOf(-1) }
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
            KoanText(
                line.text.ifEmpty { " " },
                Modifier.fillMaxWidth().clickable(remember { MutableInteractionSource() }, indication = null, enabled = durationMs > 0) {
                    app.player.seek((line.timeSecs * 1000).toLong())
                },
                role = if (index == active) KoanType.TitleSmall else KoanType.Control,
                tone = if (index == active) KoanTone.Strong else KoanTone.Muted,
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
