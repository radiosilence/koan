package cc.blit.koan.ui.pages

import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.theme.KoanButton
import cc.blit.koan.theme.KoanButtonKind
import cc.blit.koan.theme.KoanIconButton
import cc.blit.koan.theme.KoanRole
import cc.blit.koan.theme.KoanSlider
import cc.blit.koan.theme.KoanSpace
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanTheme
import cc.blit.koan.theme.KoanTone
import cc.blit.koan.ui.Sleeve
import cc.blit.koan.ui.queue.FormatBadge
import cc.blit.koan.ui.queue.LyricsView
import cc.blit.koan.ui.queue.PlayPosition
import cc.blit.koan.ui.queue.QueueFormat
import cc.blit.koan.ui.queue.TransportButtons
import cc.blit.koan.ui.queue.rememberPositionMs
import cc.blit.koan.ui.queue.sleeve

/**
 * Now playing, full screen over the tabs: the sleeve (or the lyrics in its
 * place), what is playing, the seek bar, the transport, and what the output is
 * being handed and whether the route keeps it bit-perfect.
 *
 * Shown in a dialog window so the shell needs no knowledge of it; the mini
 * player opens it and Back, the close button or a downward pull on the
 * header closes it.
 *
 * Seeking follows `PlayerModel`: dragging holds the head where the finger is
 * (`beginScrub`), letting go commits (`seek`) and the head stays until the
 * engine agrees, so the thumb does not snap back before it jumps. A track
 * still downloading can be seeked as far as it has arrived, and says so when
 * reached past.
 */
@Composable
fun NowPlayingSheet(runtime: KoanRuntime, onDismiss: () -> Unit) {
    Dialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false),
    ) {
        SheetContent(runtime, onDismiss)
    }
}

@Composable
private fun SheetContent(runtime: KoanRuntime, onDismiss: () -> Unit) {
    val playback by runtime.mirror.playback.collectAsStateWithLifecycle()
    val place by runtime.player.place.collectAsStateWithLifecycle()
    val notice by runtime.player.lastNotice.collectAsStateWithLifecycle()
    var lyrics by rememberSaveable { mutableStateOf(false) }
    val entry = playback.entry
    Column(
        Modifier
            .fillMaxSize()
            .background(KoanTheme.colors.bg)
            .safeDrawingPadding()
            .padding(horizontal = KoanSpace.page),
        verticalArrangement = Arrangement.spacedBy(KoanSpace.m),
    ) {
        // Pulled down to close; the close button is the same thing for a tap.
        Row(
            Modifier
                .fillMaxWidth()
                .pointerInput(Unit) {
                    var pulled = 0f
                    detectVerticalDragGestures(
                        onDragStart = { pulled = 0f },
                        onDragEnd = { if (pulled > PULL_TO_CLOSE_PX) onDismiss() },
                    ) { _, delta -> pulled += delta }
                },
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            KoanIconButton("chevron-down", "Close", onDismiss)
            KoanText("now playing", KoanRole.Meta, KoanTone.Muted)
            KoanIconButton(
                "lyrics", if (lyrics) "Show the sleeve" else "Show the lyrics", { lyrics = !lyrics },
                tone = if (lyrics) KoanTone.Accent else KoanTone.Ink, enabled = entry != null,
            )
        }

        BoxWithConstraints(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
            val side = minOf(maxWidth, maxHeight)
            if (lyrics) {
                LyricsView(runtime, Modifier.fillMaxSize())
            } else {
                entry?.sleeve()?.let { Sleeve(it, side, description = entry.album.ifEmpty { entry.title }) }
            }
        }

        if (entry != null) {
            Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                KoanText(QueueFormat.title(entry.title), KoanRole.TitleSm, KoanTone.Strong, data = true, maxLines = 2)
                KoanButton(
                    entry.artist, { place?.artistId?.let { runtime.navigator.openArtist(it); onDismiss() } },
                    kind = KoanButtonKind.Link, enabled = place?.artistId != null, data = true,
                )
                if (entry.album.isNotEmpty()) {
                    KoanButton(
                        entry.album, { place?.albumId?.let { runtime.navigator.openAlbum(it); onDismiss() } },
                        kind = KoanButtonKind.Link, enabled = place?.albumId != null, data = true,
                    )
                }
            }
        }

        SeekBar(runtime)
        TransportButtons(runtime, full = true, modifier = Modifier.fillMaxWidth())
        notice?.let {
            KoanButton(it, { runtime.player.dismissNotice() }, kind = KoanButtonKind.Text, data = true)
        }
        FormatBadge(runtime, Modifier.padding(bottom = KoanSpace.l))
    }
}

private const val PULL_TO_CLOSE_PX = 160f

/** The slider with the times under it. The slider reads the position in its draw block; the labels tick on their own. */
@Composable
private fun SeekBar(runtime: KoanRuntime) {
    val player = runtime.player
    val playback by runtime.mirror.playback.collectAsStateWithLifecycle()
    val seekableMs by runtime.mirror.seekableMs.collectAsStateWithLifecycle()
    val duration = playback.durationMs.toLong()
    val position = rememberPositionMs(runtime, everyFrame = true)
    val reach = if (duration <= 0 || seekableMs >= duration) 1f else (seekableMs.toFloat() / duration)
    Column {
        KoanSlider(
            progress = { PlayPosition.fraction(position.value, duration) },
            onScrub = { player.beginScrub(it.toDouble()) },
            // Says why, itself, when the track cannot be seeked yet.
            onCommit = { player.seek(it.toDouble()) },
            available = reach,
            enabled = playback.entry != null && duration > 0,
            description = "Seek",
        )
        TimeLabels(runtime, duration)
    }
}

/** Elapsed and remaining. Its own composable, so the half-second tick recomposes two labels and nothing else. */
@Composable
private fun TimeLabels(runtime: KoanRuntime, duration: Long) {
    val label = rememberPositionMs(runtime, everyFrame = false, tickMs = 500)
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
        KoanText(QueueFormat.clock(label.value), KoanRole.Fine, KoanTone.Muted, data = true)
        KoanText("-" + QueueFormat.clock((duration - label.value).coerceAtLeast(0)), KoanRole.Fine, KoanTone.Muted, data = true)
    }
}
