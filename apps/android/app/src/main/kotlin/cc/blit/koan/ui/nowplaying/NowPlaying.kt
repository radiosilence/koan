package cc.blit.koan.ui.nowplaying

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.EnterTransition
import androidx.compose.animation.ExitTransition
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.draggable
import androidx.compose.foundation.gestures.rememberDraggableState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.IconToggleButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import androidx.navigation3.ui.NavDisplay
import cc.blit.koan.art.Art
import cc.blit.koan.art.ArtSize
import cc.blit.koan.engine.Playhead
import cc.blit.koan.ffi.NowPlaying
import cc.blit.koan.ffi.PlayState
import cc.blit.koan.ffi.QueueItem
import cc.blit.koan.ffi.RepeatMode
import cc.blit.koan.ffi.SleepState
import cc.blit.koan.ffi.SleepTimer
import cc.blit.koan.ui.koanApp
import androidx.compose.ui.unit.sp
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.LocalKoanTokens
import coil3.compose.AsyncImage
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.serialization.Serializable

/** What is playing, over the tabs. */
@Serializable
data object NowPlayingRoute : NavKey

/**
 * Now Playing rises over the tabs and sinks back into them. Back, predictive
 * or not, is the root `NavDisplay`'s, which runs the sink against the gesture.
 */
fun EntryProviderScope<NavKey>.nowPlayingEntries() {
    entry<NowPlayingRoute>(
        metadata = NavDisplay.transitionSpec { slideInVertically { it } togetherWith ExitTransition.KeepUntilTransitionsFinished } +
            NavDisplay.popTransitionSpec { EnterTransition.None togetherWith slideOutVertically { it } } +
            NavDisplay.predictivePopTransitionSpec { EnterTransition.None togetherWith slideOutVertically { it } },
    ) { NowPlayingPage() }
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
    return if (s >= 3600) "%d:%02d:%02d".format(s / 3600, s / 60 % 60, s % 60) else "%d:%02d".format(s / 60, s % 60)
}

@Composable
private fun NowPlayingPage() {
    val app = koanApp()
    val nav = LocalNavigator.current
    val now by app.mirror.nowPlaying.collectAsStateWithLifecycle()
    val entry = now?.entry
    var lyrics by remember { mutableStateOf(false) }
    val accent = rememberRecordAccent(entry?.art(ArtSize.Thumb))

    // Pulled down past a threshold, or flung, it goes; otherwise it springs
    // back. The pop transition carries on from wherever it was let go.
    val drag = remember { Animatable(0f) }
    val scope = rememberCoroutineScope()
    val dismissAt = with(LocalDensity.current) { 160.dp.toPx() }

    // The one page that is only about the record playing, so in its colour.
    KoanTheme(koan = KoanTheme.isOn, icons = KoanTheme.icons, accent = accent) {
        Surface(
            Modifier.fillMaxSize()
                .graphicsLayer { translationY = drag.value }
                .draggable(
                    orientation = Orientation.Vertical,
                    state = rememberDraggableState { dy -> scope.launch { drag.snapTo((drag.value + dy).coerceAtLeast(0f)) } },
                    onDragStopped = { velocity ->
                        if (drag.value > dismissAt || velocity > 2500f) nav.closeNowPlaying() else drag.animateTo(0f)
                    },
                ),
        ) {
            Column(
                Modifier.fillMaxSize()
                    .background(Brush.verticalGradient(listOf(KoanTheme.look.accent.copy(alpha = 0.32f), MaterialTheme.colorScheme.surface)))
                    .safeDrawingPadding()
                    .padding(horizontal = LocalKoanTokens.current.gutter * 1.5f),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                IconButton(onClick = nav::closeNowPlaying, Modifier.padding(top = 4.dp)) {
                    KoanIcon(KoanIcons.expand, "Close")
                }
                AnimatedContent(
                    lyrics,
                    Modifier.weight(1f).fillMaxWidth(),
                    transitionSpec = { fadeIn() togetherWith fadeOut() },
                    label = "stage",
                ) { showLyrics ->
                    if (showLyrics) {
                        LyricsPanel(entry?.trackId, duration(now), Modifier.fillMaxSize())
                    } else {
                        Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Sleeve(entry) }
                    }
                }
                Titles(entry)
                SeekBar(now)
                Transport(now)
                Toggles(now, lyrics) { lyrics = !lyrics }
                Spacer(Modifier.size(8.dp))
            }
        }
    }
}

/** The track's length; the engine's reads 0 once playback stops, so then the queue row's. */
private fun duration(now: NowPlaying?) =
    now?.durationMs?.toLong()?.takeIf { it > 0 } ?: now?.entry?.durationMs?.toLong() ?: 0

@Composable
private fun Sleeve(entry: QueueItem?) {
    val modifier = Modifier.widthIn(max = 520.dp).fillMaxWidth().aspectRatio(1f).clip(RoundedCornerShape(12.dp))
    val art = entry?.art(ArtSize.Full)
    if (art != null) {
        AsyncImage(art, entry.album, modifier, contentScale = ContentScale.Crop)
    } else {
        Box(modifier.background(MaterialTheme.colorScheme.surfaceVariant))
    }
}

/** Title, artist and record, with the heart beside them. */
@Composable
private fun Titles(entry: QueueItem?) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(
                entry?.title ?: "Nothing playing",
                style = MaterialTheme.typography.titleLarge,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            entry?.let {
                Text(it.artist, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (it.album.isNotEmpty()) {
                    Text(
                        it.album,
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }
        }
        entry?.trackId?.let { TrackHeart(it) }
    }
}

/**
 * The heart for the playing track. Read from the engine when the track or
 * the library changes, and set from what the toggle answers.
 */
@Composable
private fun TrackHeart(trackId: Long) {
    val mirror = koanApp().mirror
    val library by mirror.library.collectAsStateWithLifecycle()
    var favourite by remember(trackId) { mutableStateOf<Boolean?>(null) }
    LaunchedEffect(trackId, library) {
        favourite = runCatching { trackId in mirror.engine().favouriteTrackIds() }.getOrNull()
    }
    val scope = rememberCoroutineScope()
    IconToggleButton(
        checked = favourite == true,
        enabled = favourite != null,
        onCheckedChange = {
            scope.launch { runCatching { mirror.engine().toggleFavourite(trackId) }.onSuccess { favourite = it } }
        },
    ) {
        KoanIcon(
            if (favourite == true) KoanIcons.favourited else KoanIcons.favourite,
            if (favourite == true) "Remove favourite" else "Favourite",
            color = modeTint(favourite == true),
        )
    }
}

/**
 * The seek bar. Counts on from the playhead's anchor; a drag holds the thumb
 * where it was let go until the engine sends the next anchor, so it does not
 * jump back to the old place and forward again.
 */
@Composable
private fun SeekBar(now: NowPlaying?) {
    val app = koanApp()
    val playhead by app.mirror.playhead.collectAsStateWithLifecycle()
    val position = rememberPosition(playhead)
    val duration = duration(now)
    var dragging by remember { mutableStateOf<Float?>(null) }
    var landing by remember { mutableStateOf<Float?>(null) }
    LaunchedEffect(playhead) { landing = null }
    val held = dragging ?: landing
    val fraction = held ?: if (duration > 0) (position.toFloat() / duration).coerceIn(0f, 1f) else 0f
    val shown = if (held != null) (held * duration).toLong() else position
    Column {
        Slider(
            value = fraction,
            onValueChange = { dragging = it },
            onValueChangeFinished = {
                dragging?.let {
                    landing = it
                    app.player.seek((it * duration).toLong())
                }
                dragging = null
            },
            enabled = duration > 0,
        )
        Row {
            Text(clock(shown), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.weight(1f))
            Text(
                "-" + clock(duration - shown),
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

/** Shuffle and repeat flank the three that move: they set how the queue plays rather than moving it. */
@Composable
private fun Transport(now: NowPlaying?) {
    val app = koanApp()
    val player = app.player
    val playing = now?.state == PlayState.PLAYING
    val enabled = now?.entry != null
    val shuffle = now?.shuffle == true
    val repeat = now?.repeatMode ?: RepeatMode.OFF
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.SpaceBetween) {
        IconButton(
            onClick = { player.setShuffle(!shuffle) },
            Modifier.semantics { stateDescription = if (shuffle) "On" else "Off" },
            enabled = enabled,
        ) {
            KoanIcon(KoanIcons.shuffle, "Shuffle", color = modeTint(shuffle))
        }
        IconButton(onClick = player::previous, enabled = enabled) {
            KoanIcon(KoanIcons.previous, "Previous", size = 30.sp)
        }
        FilledIconButton(onClick = player::togglePlayPause, Modifier.size(72.dp), enabled = enabled) {
            if (now?.waiting == true) {
                CircularProgressIndicator(Modifier.size(32.dp), color = MaterialTheme.colorScheme.onPrimary, strokeWidth = 3.dp)
            } else {
                KoanIcon(
                    if (playing) KoanIcons.pause else KoanIcons.play,
                    if (playing) "Pause" else "Play",
                    size = 34.sp,
                    color = MaterialTheme.colorScheme.onPrimary,
                )
            }
        }
        IconButton(onClick = player::next, enabled = enabled) {
            KoanIcon(KoanIcons.next, "Next", size = 30.sp)
        }
        IconButton(
            onClick = { app.mirror.launch { setRepeat(nextRepeat(repeat)) } },
            Modifier.semantics {
                stateDescription = when (repeat) {
                    RepeatMode.OFF -> "Off"
                    RepeatMode.QUEUE -> "Queue"
                    RepeatMode.ONE -> "One track"
                }
            },
            enabled = enabled,
        ) {
            KoanIcon(
                if (repeat == RepeatMode.ONE) KoanIcons.repeatOne else KoanIcons.repeatQueue,
                "Repeat",
                color = modeTint(repeat != RepeatMode.OFF),
            )
        }
    }
}

/** Off, the queue, one track: one press steps to the next. */
internal fun nextRepeat(mode: RepeatMode) = when (mode) {
    RepeatMode.OFF -> RepeatMode.QUEUE
    RepeatMode.QUEUE -> RepeatMode.ONE
    RepeatMode.ONE -> RepeatMode.OFF
}

@Composable
private fun modeTint(on: Boolean) = if (on) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant

/** The lyrics, the format and the sleep timer: what is switched on for listening, and what the output is fed. */
@Composable
private fun Toggles(now: NowPlaying?, lyrics: Boolean, toggleLyrics: () -> Unit) {
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        IconButton(onClick = toggleLyrics, Modifier.semantics { stateDescription = if (lyrics) "Shown" else "Hidden" }) {
            KoanIcon(KoanIcons.lyrics, if (lyrics) "Show artwork" else "Show lyrics", color = modeTint(lyrics))
        }
        Spacer(Modifier.weight(1f))
        now?.format?.let { format ->
            Surface(
                shape = RoundedCornerShape(6.dp),
                color = Color.Transparent,
                border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
            ) {
                Text(
                    quality(format),
                    Modifier.padding(horizontal = 8.dp, vertical = 3.dp),
                    style = MaterialTheme.typography.labelMedium,
                    maxLines = 1,
                )
            }
        }
        Spacer(Modifier.weight(1f))
        SleepButton(now?.sleep, now?.sleepFading == true)
    }
}

/**
 * The sleep timer: the fixed choices, and while one is set what is left
 * beside the moon. The engine says when it goes off rather than what is left,
 * so the countdown is worked out here, once a second while it shows.
 */
@Composable
private fun SleepButton(sleep: SleepState?, fading: Boolean) {
    val mirror = koanApp().mirror
    var open by remember { mutableStateOf(false) }
    Box {
        TextButton(onClick = { open = true }, Modifier.semantics { contentDescription = "Sleep timer" }) {
            KoanIcon(if (sleep == null) KoanIcons.sleep else KoanIcons.sleepSet, null, size = 16.sp, color = modeTint(sleep != null))
            val label = when {
                fading -> "Fading"
                sleep is SleepState.At -> countdown(sleep.unixMs.toLong())
                sleep is SleepState.EndOfTrack -> "Track"
                sleep is SleepState.EndOfRecord -> "Record"
                else -> null
            }
            label?.let {
                Spacer(Modifier.size(4.dp))
                Text(it, style = MaterialTheme.typography.labelLarge)
            }
        }
        DropdownMenu(open, onDismissRequest = { open = false }) {
            fun set(timer: SleepTimer) {
                open = false
                mirror.launch { setSleepTimer(timer) }
            }
            listOf(15, 30, 45, 60).forEach { minutes ->
                DropdownMenuItem(text = { Text("$minutes minutes") }, onClick = { set(SleepTimer.After(minutes.toUInt())) })
            }
            HorizontalDivider()
            DropdownMenuItem(text = { Text("End of track") }, onClick = { set(SleepTimer.EndOfTrack) })
            DropdownMenuItem(text = { Text("End of record") }, onClick = { set(SleepTimer.EndOfRecord) })
            if (sleep != null) {
                HorizontalDivider()
                DropdownMenuItem(
                    text = { Text("Cancel sleep timer", color = MaterialTheme.colorScheme.error) },
                    onClick = {
                        open = false
                        mirror.launch { cancelSleepTimer() }
                    },
                )
            }
        }
    }
}

@Composable
private fun countdown(atUnixMs: Long): String {
    var left by remember(atUnixMs) { mutableLongStateOf(atUnixMs - System.currentTimeMillis()) }
    LaunchedEffect(atUnixMs) {
        while (left > 0) {
            delay(left % 1000 + 1)
            left = atUnixMs - System.currentTimeMillis()
        }
    }
    return clock(left + 999)
}
