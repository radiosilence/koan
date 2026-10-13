package cc.blit.koan.ui.queue

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.State
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.RectangleShape
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.engine.KoanProcess
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.service.BitPerfectStatus
import cc.blit.koan.state.Playhead
import cc.blit.koan.theme.KoanIconButton
import cc.blit.koan.theme.KoanRole
import cc.blit.koan.theme.KoanShape
import cc.blit.koan.theme.KoanSpace
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanTheme
import cc.blit.koan.theme.KoanTone
import kotlinx.coroutines.delay
import uniffi.koan_ffi.PlayState
import uniffi.koan_ffi.RepeatMode

/** Where the head is, as the transport shows it. */
object PlayPosition {
    /**
     * The position in ms: where a thumb is being held or a seek is settling
     * (`scrubbing`, a 0 to 1 fraction) if anything is, otherwise the
     * playhead's own reckoning, capped at the track's length.
     */
    fun positionMs(playhead: Playhead, scrubbing: Double?, durationMs: Long, nowNanos: Long): Long =
        if (scrubbing != null && durationMs > 0) (scrubbing * durationMs).toLong() else playhead.at(nowNanos, durationMs)

    fun fraction(positionMs: Long, durationMs: Long): Float =
        if (durationMs <= 0) 0f else (positionMs.toDouble() / durationMs).toFloat().coerceIn(0f, 1f)
}

/**
 * The playhead as a position in ms that moves while the music does: every
 * frame for a bar that draws smoothly, or every [tickMs] for a label. Nothing
 * ticks while paused or off screen; the anchor from the engine says where the
 * head is and this runs it forward.
 *
 * The state is meant to be read inside a draw block or a tiny text composable,
 * so that a tick never recomposes anything bigger.
 */
@Composable
fun rememberPositionMs(runtime: KoanRuntime, everyFrame: Boolean, tickMs: Long = 250): State<Long> {
    val playhead by runtime.mirror.playhead.collectAsStateWithLifecycle()
    val playback by runtime.mirror.playback.collectAsStateWithLifecycle()
    val scrubbing by runtime.player.scrubbing.collectAsStateWithLifecycle()
    val position = remember { mutableLongStateOf(0L) }
    val duration = playback.durationMs.toLong()
    LaunchedEffect(playhead, duration, scrubbing) {
        fun now() = PlayPosition.positionMs(playhead, scrubbing, duration, System.nanoTime())
        position.longValue = now()
        if (scrubbing == null && playhead.playing && duration > 0) {
            while (true) {
                if (everyFrame) withFrameNanos { } else delay(tickMs)
                position.longValue = now()
            }
        }
    }
    return position
}

/** The transport: previous, play or pause, next, and with [full] shuffle and repeat either side. */
@Composable
fun TransportButtons(runtime: KoanRuntime, full: Boolean, modifier: Modifier = Modifier) {
    val playback by runtime.mirror.playback.collectAsStateWithLifecycle()
    val player = runtime.player
    val playing = playback.state == PlayState.PLAYING
    Row(modifier, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.SpaceEvenly) {
        if (full) {
            KoanIconButton(
                "shuffle", if (playback.shuffle) "Shuffle on" else "Shuffle off", { player.toggleShuffle() },
                tone = if (playback.shuffle) KoanTone.Accent else KoanTone.Muted,
            )
        }
        KoanIconButton("previous", "Previous", { player.previous() })
        KoanIconButton(
            if (playing) "pause" else "play", if (playing) "Pause" else "Play", { player.togglePlayPause() },
            outlined = full,
        )
        KoanIconButton("next", "Next", { player.next() })
        if (full) {
            val repeat = playback.repeatMode
            KoanIconButton(
                if (repeat == RepeatMode.ONE) "repeat-one" else "repeat",
                when (repeat) {
                    RepeatMode.OFF -> "Repeat off"
                    RepeatMode.QUEUE -> "Repeat the queue"
                    RepeatMode.ONE -> "Repeat one"
                },
                { player.cycleRepeat() },
                tone = if (repeat == RepeatMode.OFF) KoanTone.Muted else KoanTone.Accent,
            )
        }
    }
}

/**
 * What the output is being handed, and whether the route keeps it that way.
 *
 * The first line is the source ("FLAC 24/96", with a rate the device refused
 * appended). The second is the route's verdict, judged from what the output
 * stream was *granted* and read back (never from what was asked for): only a
 * USB DAC can be bit-perfect on Android; speakers, wired headphones and
 * Bluetooth go through the system mixer and are resampled. It is outlined in
 * the accent when the route is bit-perfect and in `muted` when it is not, and
 * says why not in words.
 */
@Composable
fun FormatBadge(runtime: KoanRuntime, modifier: Modifier = Modifier) {
    val playback by runtime.mirror.playback.collectAsStateWithLifecycle()
    val route by KoanProcess.bitPerfect.collectAsStateWithLifecycle()
    val format = playback.format ?: return
    val verdict = BadgeText.verdict(route)
    val exact = route is BitPerfectStatus.Exclusive || route is BitPerfectStatus.MixerBitPerfect
    Column(modifier, verticalArrangement = Arrangement.spacedBy(KoanSpace.xs)) {
        Column(
            Modifier
                .border(BorderStroke(KoanShape.hairline, if (exact) KoanTheme.colors.accent else KoanTheme.colors.muted), RectangleShape)
                .padding(horizontal = KoanSpace.m, vertical = KoanSpace.xs),
        ) {
            KoanText(QueueFormat.quality(format), KoanRole.Meta, if (exact) KoanTone.Accent else KoanTone.Ink, data = true)
            KoanText(verdict, KoanRole.Fine, if (exact) KoanTone.Accent else KoanTone.Muted, data = true)
        }
        KoanText(QueueFormat.outputExplanation(format), KoanRole.Fine, KoanTone.Muted, data = true, maxLines = 3)
    }
}

/** The route's verdict, in words. */
object BadgeText {
    fun verdict(status: BitPerfectStatus): String = when (status) {
        BitPerfectStatus.Exclusive -> "bit-perfect · exclusive to the DAC"
        BitPerfectStatus.MixerBitPerfect -> "bit-perfect · through the system mixer"
        is BitPerfectStatus.No -> "not bit-perfect · ${status.reason.replaceFirstChar { it.lowercase() }}"
    }
}
