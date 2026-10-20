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
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.draggable
import androidx.compose.foundation.gestures.rememberDraggableState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
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
import cc.blit.koan.ffi.Track
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.library.AlbumRoute
import cc.blit.koan.ui.library.Cover
import cc.blit.koan.ui.library.NameLine
import cc.blit.koan.ui.library.Playable
import cc.blit.koan.ui.library.PlayableMenu
import cc.blit.koan.ui.library.TrackHeart
import cc.blit.koan.ui.library.artistCredit
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.settings.OutputButton
import cc.blit.koan.ui.settings.PresetButton
import cc.blit.koan.ui.settings.canChooseOutput
import cc.blit.koan.ui.settings.dspModel
import cc.blit.koan.ui.settings.outputLabel
import cc.blit.koan.ui.settings.presetSummary
import cc.blit.koan.ui.settings.PlayOnButton
import cc.blit.koan.ui.theme.KoanBadge
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanDivider
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanMenu
import cc.blit.koan.ui.theme.KoanMenuItem
import cc.blit.koan.ui.theme.KoanSlider
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import cc.blit.koan.ui.theme.recordWash
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
fun rememberPosition(playhead: Playhead, durationMs: Long = 0): Long {
    var position by remember { mutableLongStateOf(playhead.positionAt(durationMs = durationMs)) }
    LaunchedEffect(playhead, durationMs) {
        position = playhead.positionAt(durationMs = durationMs)
        while (playhead.playing && (durationMs <= 0 || position < durationMs)) {
            delay(1000 - position % 1000)
            position = playhead.positionAt(durationMs = durationMs)
        }
    }
    return position
}

/** A queue row's cover: its record's, which lists already hold, else its own. */
fun QueueItem.art(size: ArtSize) = albumId?.let { Art.album(it, size) } ?: trackId?.let { Art.track(it, size) }

/** The track's length. The engine's reads 0 once playback stops, so then the queue row's. */
fun NowPlaying?.length() = this?.durationMs?.toLong()?.takeIf { it > 0 } ?: this?.entry?.durationMs?.toLong() ?: 0

fun clock(ms: Long): String {
    val s = ms.coerceAtLeast(0) / 1000
    return if (s >= 3600) "%d:%02d:%02d".format(s / 3600, s / 60 % 60, s % 60) else "%d:%02d".format(s / 60, s % 60)
}

/**
 * The record, full screen: the sleeve (or its words), what is playing, the
 * seek bar, the transport, and what is switched on for listening. The page
 * is in the record's colour already: `KoanRoot` tints the app with it.
 */
@Composable
private fun NowPlayingPage() {
    val app = koanApp()
    val nav = LocalNavigator.current
    val now by app.mirror.nowPlaying.collectAsStateWithLifecycle()
    val entry = now?.entry
    var lyrics by remember { mutableStateOf(false) }

    // Pulled down past a threshold, or flung, it goes; otherwise it springs
    // back. The pop transition carries on from wherever it was let go.
    val drag = remember { Animatable(0f) }
    val scope = rememberCoroutineScope()
    val dismissAt = with(LocalDensity.current) { 160.dp.toPx() }

    BoxWithConstraints(
        Modifier.fillMaxSize()
            .graphicsLayer { translationY = drag.value }
            // Opaque, as it is dragged down over the page beneath.
            .recordWash()
            .draggable(
                orientation = Orientation.Vertical,
                state = rememberDraggableState { dy -> scope.launch { drag.snapTo((drag.value + dy).coerceAtLeast(0f)) } },
                onDragStopped = { velocity ->
                    if (drag.value > dismissAt || velocity > 2500f) nav.closeNowPlaying() else drag.animateTo(0f)
                },
            )
            .safeDrawingPadding()
            .padding(horizontal = KoanSpace.page),
    ) {
        val stage = @Composable { modifier: Modifier ->
            AnimatedContent(lyrics, modifier, transitionSpec = { fadeIn() togetherWith fadeOut() }, label = "stage") { showLyrics ->
                if (showLyrics) {
                    LyricsPanel(entry?.trackId, now.length(), Modifier.fillMaxSize())
                } else {
                    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Sleeve(entry) }
                }
            }
        }
        val controls = @Composable {
            Titles(entry)
            SeekBar(now)
            Transport(now)
            Toggles(now, lyrics) { lyrics = !lyrics }
        }
        // Wider than tall, the sleeve takes the left and the controls the
        // right, as on an iPad: stacked, the sleeve would have no height left.
        if (maxWidth > maxHeight) {
            Row(Modifier.fillMaxSize(), horizontalArrangement = Arrangement.spacedBy(KoanSpace.xl)) {
                stage(Modifier.weight(1f).fillMaxHeight().padding(vertical = KoanSpace.m))
                Column(
                    Modifier.weight(1f).fillMaxHeight(),
                    verticalArrangement = Arrangement.spacedBy(KoanSpace.m, Alignment.CenterVertically),
                ) {
                    GlyphButton(KoanIcons.expand, "Close", nav::closeNowPlaying)
                    controls()
                }
            }
        } else {
            Column(Modifier.fillMaxSize(), verticalArrangement = Arrangement.spacedBy(KoanSpace.m)) {
                GlyphButton(KoanIcons.expand, "Close", nav::closeNowPlaying, Modifier.padding(top = KoanSpace.xs))
                stage(Modifier.weight(1f).fillMaxWidth())
                controls()
                Spacer(Modifier.size(KoanSpace.s))
            }
        }
    }
}

/** A glyph in the theme's icon button, coloured by `tone`: the accent for a mode that is on. */
@Composable
private fun GlyphButton(
    icon: Int,
    label: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    tone: KoanTone = KoanTone.Ink,
    size: TextUnit = 18.sp,
    enabled: Boolean = true,
) {
    KoanButton(onClick, modifier, kind = KoanButtonKind.Icon, enabled = enabled) {
        KoanIcon(icon, label, size = size, color = KoanTheme.color(tone))
    }
}

/** Square in the theme, as every sleeve is; rounded in the platform's look. */
@Composable
private fun Sleeve(entry: QueueItem?) {
    val shape = if (KoanTheme.isOn) RectangleShape else RoundedCornerShape(12.dp)
    // Square, as large as fits either way.
    val modifier = Modifier.widthIn(max = 520.dp).aspectRatio(1f, matchHeightConstraintsFirst = true).clip(shape)
    Cover(entry?.art(ArtSize.Full), modifier, entry?.album)
}

/**
 * Title, then the artist and the record as links to their pages, with the
 * heart and the track's menu beside them.
 */
@Composable
private fun Titles(entry: QueueItem?) {
    val app = koanApp()
    val mirror = app.mirror
    val nav = LocalNavigator.current
    val library by mirror.library.collectAsStateWithLifecycle()
    val trackId = entry?.trackId
    // The library's row for the track: what its menu acts on and its links lead to.
    val track by produceState<Track?>(null, trackId, library) {
        value = trackId?.let { id -> runCatching { mirror.engine().track(id) }.getOrNull() }
    }
    // The titles open the track's info sheet, the artist and the record being links of their own.
    val showInfo = LocalShowTrackInfo.current
    var menu by remember { mutableStateOf(false) }
    // A play still finding its tracks, named from the tap rather than leaving
    // the paused track it replaces on show.
    val resolving by app.player.resolving.collectAsStateWithLifecycle()
    resolving?.let { name ->
        Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
            KoanText(name, role = KoanType.TitleSmall, tone = KoanTone.Strong, maxLines = 1)
            KoanText(KoanTheme.label("Loading…"), tone = KoanTone.Muted, maxLines = 1)
        }
        return
    }
    Row(verticalAlignment = Alignment.CenterVertically) {
        Column(
            Modifier.weight(1f).link(trackId != null, label = "Track info") { trackId?.let(showInfo) },
            verticalArrangement = Arrangement.spacedBy(2.dp),
        ) {
            KoanText(entry?.title ?: "Nothing playing", role = KoanType.TitleSmall, tone = KoanTone.Strong, maxLines = 1)
            entry?.let {
                val albumId = track?.albumId ?: it.albumId
                NameLine(
                    listOf(artistCredit(it.artistCredit, it.artist, it.artistId)),
                    role = KoanType.Body,
                    tone = KoanTone.Ink,
                )
                if (it.album.isNotEmpty()) {
                    KoanText(
                        it.album,
                        Modifier.link(albumId != null) { nav.push(AlbumRoute(albumId!!, trackId)) },
                        role = KoanType.Meta,
                        tone = KoanTone.Muted,
                        maxLines = 1,
                    )
                }
            }
        }
        trackId?.let { TrackHeart(it) }
        track?.let {
            Box {
                KoanButton("More", { menu = true }, kind = KoanButtonKind.Icon, icon = KoanIcons.more)
                PlayableMenu(Playable.OfTrack(it), menu) { menu = false }
            }
        }
    }
}

/** A line of text that opens a page, without a ripple: the theme presses nothing but buttons. */
private fun Modifier.link(enabled: Boolean, label: String? = null, onClick: () -> Unit) =
    if (enabled) clickable(interactionSource = null, indication = null, onClickLabel = label, role = Role.Button, onClick = onClick) else this

/**
 * The seek bar. Counts on from the playhead's anchor; a drag holds the thumb
 * where it was let go until the engine sends the next anchor, so it does not
 * jump back to the old place and forward again.
 */
@Composable
private fun SeekBar(now: NowPlaying?) {
    val app = koanApp()
    val playhead by app.mirror.playhead.collectAsStateWithLifecycle()
    val duration = now.length()
    val position = rememberPosition(playhead, duration)
    var dragging by remember { mutableStateOf<Float?>(null) }
    var landing by remember { mutableStateOf<Float?>(null) }
    LaunchedEffect(playhead) { landing = null }
    val held = dragging ?: landing
    val fraction = held ?: if (duration > 0) (position.toFloat() / duration).coerceIn(0f, 1f) else 0f
    val shown = if (held != null) (held * duration).toLong() else position
    KoanSlider(
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
        low = { KoanText(clock(shown), role = KoanType.Fine, tone = KoanTone.Muted) },
        high = { KoanText("-" + clock(duration - shown), role = KoanType.Fine, tone = KoanTone.Muted) },
    )
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
    val resolving by player.resolving.collectAsStateWithLifecycle()
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.SpaceBetween) {
        GlyphButton(
            KoanIcons.shuffle,
            "Shuffle",
            { player.setShuffle(!shuffle) },
            Modifier.semantics { stateDescription = if (shuffle) "On" else "Off" },
            tone = if (shuffle) KoanTone.Accent else KoanTone.Muted,
            enabled = enabled,
        )
        GlyphButton(KoanIcons.previous, "Previous", player::previous, size = 24.sp, enabled = enabled)
        KoanButton(player::togglePlayPause, kind = KoanButtonKind.IconOutlined, enabled = enabled) {
            Box(Modifier.size(44.dp), contentAlignment = Alignment.Center) {
                if (now?.waiting == true || resolving != null) {
                    CircularProgressIndicator(Modifier.size(24.dp), color = KoanTheme.color(KoanTone.Ink), strokeWidth = 2.dp)
                } else {
                    KoanIcon(if (playing) KoanIcons.pause else KoanIcons.play, if (playing) "Pause" else "Play", size = 28.sp)
                }
            }
        }
        GlyphButton(KoanIcons.next, "Next", player::next, size = 24.sp, enabled = enabled)
        GlyphButton(
            if (repeat == RepeatMode.ONE) KoanIcons.repeatOne else KoanIcons.repeatQueue,
            "Repeat",
            { app.mirror.launch { setRepeat(nextRepeat(repeat)) } },
            Modifier.semantics {
                stateDescription = when (repeat) {
                    RepeatMode.OFF -> "Off"
                    RepeatMode.QUEUE -> "Queue"
                    RepeatMode.ONE -> "One track"
                }
            },
            tone = if (repeat != RepeatMode.OFF) KoanTone.Accent else KoanTone.Muted,
            enabled = enabled,
        )
    }
}

/** Off, the queue, one track: one press steps to the next. */
internal fun nextRepeat(mode: RepeatMode) = when (mode) {
    RepeatMode.OFF -> RepeatMode.QUEUE
    RepeatMode.QUEUE -> RepeatMode.ONE
    RepeatMode.ONE -> RepeatMode.OFF
}

/**
 * The lyrics, the format and the sleep timer: what is switched on for
 * listening, and what the output is fed. Under them, as iOS's row of pills,
 * where the music plays, what through, and this phone's EQ preset: each its
 * icon and name as one button, centred, a long name cut short rather than
 * pushed into its neighbour. Each shows only where it is a choice: Play on
 * with another device to play on, Output with an output to pick, and the
 * preset while this phone plays and has presets. Another device's presets are
 * chosen in Output, by its outputs.
 */
@Composable
private fun Toggles(now: NowPlaying?, lyrics: Boolean, toggleLyrics: () -> Unit) {
    val mirror = koanApp().mirror
    val devices by mirror.devices.collectAsStateWithLifecycle()
    val outputs by mirror.outputs.collectAsStateWithLifecycle()
    val dsp = dspModel()
    val eq by dsp.state.collectAsStateWithLifecycle()
    LaunchedEffect(dsp) { dsp.reload() }
    val controlling = devices.target != null
    // One search for renderers on opening, as iOS does: those that answer
    // are what makes Output worth showing here.
    LaunchedEffect(controlling) { if (!controlling) mirror.engine().searchRenderers() }
    val playOn = controlling || devices.devices.isNotEmpty()
    val output = canChooseOutput(outputs, controlling)
    val named = outputLabel(outputs) != null
    val o = eq.overview?.takeIf { !controlling && it.device != null && (it.profiles.isNotEmpty() || it.preset != null) }
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(KoanSpace.xs)) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            GlyphButton(
                KoanIcons.lyrics,
                if (lyrics) "Show artwork" else "Show lyrics",
                toggleLyrics,
                tone = if (lyrics) KoanTone.Accent else KoanTone.Muted,
            )
            Spacer(Modifier.weight(1f))
            now?.format?.let { format ->
                // In the accent while the device gets the samples untouched.
                val perfect = bitPerfect(format)
                KoanBadge(
                    quality(format),
                    Modifier.semantics { if (perfect) stateDescription = "Bit-perfect" },
                    accent = perfect,
                )
            }
            Spacer(Modifier.weight(1f))
            SleepButton(now?.sleep, now?.sleepFading == true)
        }
        if (playOn || output || o != null) {
            Row(
                Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(KoanSpace.l, Alignment.CenterHorizontally),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                // Only a named button shares the room, so a bare glyph never
                // takes a share a name could use.
                if (playOn) PlayOnButton(if (controlling) Modifier.weight(1f, fill = false) else Modifier)
                if (output) OutputButton(if (named) Modifier.weight(1f, fill = false) else Modifier)
                o?.let {
                    PresetButton(
                        presetSummary(it),
                        it.profiles.filter { p -> p.preset }.map { p -> p.name },
                        it.preset,
                        tinted = now?.format?.dsp != null,
                        choose = dsp::applyPreset,
                        modifier = Modifier.weight(1f, fill = false),
                    )
                }
            }
        }
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
        KoanButton({ open = true }, Modifier.semantics { contentDescription = "Sleep timer" }, kind = KoanButtonKind.Text) {
            val tone = if (sleep == null) KoanTone.Muted else KoanTone.Accent
            KoanIcon(if (sleep == null) KoanIcons.sleep else KoanIcons.sleepSet, null, color = KoanTheme.color(tone))
            val label = when {
                fading -> KoanTheme.label("Fading")
                sleep is SleepState.At -> countdown(sleep.unixMs.toLong())
                sleep is SleepState.EndOfTrack -> KoanTheme.label("Track")
                sleep is SleepState.EndOfRecord -> KoanTheme.label("Record")
                else -> null
            }
            label?.let { KoanText(it, role = KoanType.Control, tone = tone) }
        }
        KoanMenu(open, onDismissRequest = { open = false }) {
            fun set(timer: SleepTimer) {
                open = false
                mirror.launch { setSleepTimer(timer) }
            }
            listOf(15, 30, 45, 60).forEach { minutes ->
                KoanMenuItem("$minutes minutes") { set(SleepTimer.After(minutes.toUInt())) }
            }
            KoanDivider()
            KoanMenuItem("End of track") { set(SleepTimer.EndOfTrack) }
            KoanMenuItem("End of record") { set(SleepTimer.EndOfRecord) }
            if (sleep != null) {
                KoanDivider()
                KoanMenuItem("Cancel sleep timer", destructive = true) {
                    open = false
                    mirror.launch { cancelSleepTimer() }
                }
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
