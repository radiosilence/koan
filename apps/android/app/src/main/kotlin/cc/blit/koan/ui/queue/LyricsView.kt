package cc.blit.koan.ui.queue

import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.engine.QueueSession
import cc.blit.koan.state.LyricsTimeline
import cc.blit.koan.theme.KoanRole
import cc.blit.koan.theme.KoanSpace
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanTone
import kotlinx.coroutines.delay
import uniffi.koan_ffi.Lyrics

/**
 * Synced lyrics, highlighted against playback position, in the sleeve's place
 * on the now-playing sheet. The engine hands back already-parsed LRC lines, so
 * this only picks the current one and scrolls to it.
 *
 * The words come from [cc.blit.koan.state.LyricsModel], which asks the engine
 * off the main thread and only while this is composed; nothing here calls it.
 */
@Composable
fun LyricsView(runtime: KoanRuntime, modifier: Modifier = Modifier) {
    val model = remember(runtime) { QueueSession.lyrics(runtime) }
    LaunchedEffect(model) { model.follow() }
    val state by model.state.collectAsStateWithLifecycle()
    val playback by runtime.mirror.playback.collectAsStateWithLifecycle()
    val lyrics = state.lyrics?.takeIf { state.trackId == playback.entry?.trackId }
    Column(modifier) {
        Row(Modifier.fillMaxWidth().padding(bottom = KoanSpace.s), horizontalArrangement = Arrangement.SpaceBetween) {
            KoanText("lyrics", KoanRole.Body, KoanTone.Strong)
            if (state.loading) {
                KoanText("looking", KoanRole.Fine, KoanTone.Muted)
            } else {
                lyrics?.source?.let { KoanText(it, KoanRole.Fine, KoanTone.Muted, data = true) }
            }
        }
        when {
            lyrics != null && lyrics.lines.isNotEmpty() -> SyncedLyrics(runtime, lyrics)
            lyrics != null && lyrics.content.isNotEmpty() -> {
                Box(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
                    KoanText(lyrics.content, KoanRole.Control, KoanTone.Ink, data = true, maxLines = Int.MAX_VALUE)
                }
            }
            state.loading -> Unit
            else -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                KoanText(
                    if (playback.entry == null) "Nothing playing" else "No lyrics found", KoanRole.Meta, KoanTone.Muted,
                )
            }
        }
    }
}

/**
 * The synced list. The line being sung is found by sleeping until the next
 * timestamp, not by asking where the playhead is: one wake per line, none
 * while paused, and a seek restarts it because the anchor it reads moved.
 * Tapping a line seeks to it.
 */
@Composable
private fun SyncedLyrics(runtime: KoanRuntime, lyrics: Lyrics) {
    val playhead by runtime.mirror.playhead.collectAsStateWithLifecycle()
    val active = remember(lyrics) { mutableIntStateOf(-1) }
    val listState = rememberLazyListState()
    LaunchedEffect(lyrics, playhead) {
        while (true) {
            val position = playhead.at(System.nanoTime()) / 1000.0
            active.intValue = LyricsTimeline.currentIndex(lyrics.lines, position)
            if (!playhead.playing) break
            delay(LyricsTimeline.millisToNext(lyrics.lines, position) ?: break)
        }
    }
    LaunchedEffect(lyrics) {
        snapshotFlow { active.intValue }.collect { index ->
            if (index >= 0) listState.animateScrollToItem(index, scrollOffset = -(listState.layoutInfo.viewportSize.height / 2 - LINE_ALLOWANCE_PX))
        }
    }
    LazyColumn(state = listState, modifier = Modifier.fillMaxSize(), verticalArrangement = Arrangement.spacedBy(KoanSpace.m)) {
        itemsIndexed(lyrics.lines, key = { index, _ -> index }, contentType = { _, _ -> "line" }) { index, line ->
            val current by remember(index) { derivedStateOf { active.intValue == index } }
            KoanText(
                line.text.ifEmpty { " " },
                if (current) KoanRole.TitleSm else KoanRole.Control,
                if (current) KoanTone.Strong else KoanTone.Muted,
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable(interactionSource = remember { MutableInteractionSource() }, indication = null, role = Role.Button) {
                        runtime.player.seekTo((line.timeSecs * 1000).toLong())
                    },
                data = true, maxLines = 4,
            )
        }
        item(key = "tail", contentType = "tail") { Box(Modifier.padding(vertical = 60.dp)) }
    }
}

private const val LINE_ALLOWANCE_PX = 40
