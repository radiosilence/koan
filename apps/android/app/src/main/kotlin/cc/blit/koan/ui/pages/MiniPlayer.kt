package cc.blit.koan.ui.pages

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.theme.KoanIconButton
import cc.blit.koan.theme.KoanRole
import cc.blit.koan.theme.KoanRule
import cc.blit.koan.theme.KoanSpace
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanTheme
import cc.blit.koan.theme.KoanTone
import cc.blit.koan.ui.Sleeve
import cc.blit.koan.ui.queue.PlayPosition
import cc.blit.koan.ui.queue.QueueFormat
import cc.blit.koan.ui.queue.rememberPositionMs
import cc.blit.koan.ui.queue.sleeve
import uniffi.koan_ffi.PlayState

/**
 * The mini player the shell puts directly above the tab bar, with its own top
 * rule: the playhead as a 2 dp accent line, the sleeve, title and transport,
 * opening the now-playing sheet. Draws nothing with nothing queued.
 *
 * The line is drawn from the position state inside a draw block, ticking four
 * times a second while the music plays and not at all when it does not, so it
 * recomposes nothing. The sheet it opens is [NowPlayingSheet].
 */
@Composable
fun MiniPlayer(runtime: KoanRuntime) {
    val playback by runtime.mirror.playback.collectAsStateWithLifecycle()
    val resolving by runtime.player.resolving.collectAsStateWithLifecycle()
    var open by rememberSaveable { mutableStateOf(false) }
    val entry = playback.entry
    val playing = playback.state == PlayState.PLAYING
    val position = rememberPositionMs(runtime, everyFrame = false, tickMs = 250)
    val duration = playback.durationMs.toLong()
    val accent = KoanTheme.colors.accent
    if (entry == null) return
    Column(Modifier.fillMaxWidth().background(KoanTheme.colors.bg)) {
        KoanRule()
        Box(
            Modifier
                .fillMaxWidth()
                .height(2.dp)
                .drawBehind {
                    val f = PlayPosition.fraction(position.value, duration)
                    drawRect(accent, Offset.Zero, Size(size.width * f, size.height))
                },
        )
        Row(
            Modifier
                .fillMaxWidth()
                .clickable(interactionSource = remember { MutableInteractionSource() }, indication = null, role = Role.Button) { open = true }
                .padding(start = KoanSpace.page, end = KoanSpace.xs, top = KoanSpace.xs, bottom = KoanSpace.xs),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(KoanSpace.m),
        ) {
            entry.sleeve()?.let { Sleeve(it, 40.dp) }
            Column(Modifier.weight(1f)) {
                KoanText(QueueFormat.title(resolving ?: entry.title), KoanRole.Meta, KoanTone.Strong, data = true)
                KoanText(
                    if (resolving != null) "loading" else entry.artist, KoanRole.Fine, KoanTone.Muted,
                    data = resolving == null,
                )
            }
            KoanIconButton(
                if (playing) "pause" else "play", if (playing) "Pause" else "Play", { runtime.player.togglePlayPause() },
            )
            KoanIconButton("next", "Next", { runtime.player.next() })
        }
    }
    if (open) NowPlayingSheet(runtime, onDismiss = { open = false })
}
