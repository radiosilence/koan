package cc.blit.koan.ui.queue

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.runtime.Composable
import androidx.compose.runtime.State
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.art.ArtworkSource
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.state.PlayingLevels
import cc.blit.koan.state.QueueState
import cc.blit.koan.theme.KoanIcon
import cc.blit.koan.theme.KoanRole
import cc.blit.koan.theme.KoanSpace
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanTheme
import cc.blit.koan.theme.KoanTone
import cc.blit.koan.ui.Sleeve
import uniffi.koan_ffi.EntryStatus
import uniffi.koan_ffi.PlayState
import uniffi.koan_ffi.QueueItem

/** Which sleeve to draw for a queue item: the record wherever the library knows it, else its own file. */
fun QueueItem.sleeve(): ArtworkSource? =
    albumId?.let(ArtworkSource::Album) ?: trackId?.let(ArtworkSource::Track)

/** Row sizes: text rows and sleeve rows, the same on every list. */
internal object RowSize {
    val text = 44.dp
    val art = 56.dp
    val sleeve = 40.dp
    val headingSleeve = 52.dp
}

/**
 * An album heading: the sleeve, the record, its artist and "2007 · 11 tracks ·
 * 59:10 · FLAC". Its own row, so an album can be picked as a unit.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun HeadingRow(
    line: QueueLine.Heading,
    selected: Boolean,
    onClick: () -> Unit,
    onLongClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val group = line.group
    Row(
        modifier
            .fillMaxWidth()
            .background(if (selected) KoanTheme.colors.accent.copy(alpha = SELECTED_ALPHA) else Color.Transparent)
            .combinedClickable(
                interactionSource = remember { MutableInteractionSource() }, indication = null,
                role = Role.Button, onClick = onClick, onLongClick = onLongClick,
            )
            .padding(horizontal = KoanSpace.page, vertical = KoanSpace.s),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(KoanSpace.m),
    ) {
        group.items.firstOrNull()?.sleeve()?.let { Sleeve(it, RowSize.headingSleeve) }
            ?: Box(Modifier.size(RowSize.headingSleeve))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            KoanText(QueueFormat.title(group.title), KoanRole.Control, KoanTone.Strong, data = true, maxLines = 2)
            if (group.album.isNotEmpty()) {
                KoanText(group.albumArtist.ifEmpty { "Unknown Artist" }, KoanRole.Fine, KoanTone.Muted, data = true)
            }
            KoanText(QueueFormat.groupDetail(group), KoanRole.Fine, KoanTone.Muted, data = true)
        }
    }
}

/**
 * One track. It draws the item as the queue reads *now*: [queue] is the state
 * the page collected and never read, and each row reads its own entry through
 * `derivedStateOf`, so a track change or a download redraws the rows whose
 * item changed and not the list. (`QueueState.item` is by id; the row sent
 * with the layout stands in for an item the queue has since dropped.)
 *
 * [dragHandle] is the handle the reorder library hands the row, or null where
 * reordering is not offered.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun TrackRow(
    runtime: KoanRuntime,
    levels: PlayingLevels,
    line: QueueLine.Track,
    queue: State<QueueState>,
    favourites: State<Set<Long>>,
    selected: Boolean,
    onClick: () -> Unit,
    onLongClick: () -> Unit,
    modifier: Modifier = Modifier,
    dragHandle: (@Composable RowScope.() -> Unit)? = null,
) {
    val item by remember(line.id, queue) { derivedStateOf { queue.value.item(line.id) ?: line.item } }
    val current = item.status == EntryStatus.PLAYING
    val played = item.status == EntryStatus.PLAYED
    val art = line.artwork
    val title = when {
        current -> KoanTone.Accent
        played -> KoanTone.Muted
        else -> KoanTone.Ink
    }
    Box(modifier) {
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = if (art) RowSize.art else RowSize.text)
            .background(if (selected) KoanTheme.colors.accent.copy(alpha = SELECTED_ALPHA) else Color.Transparent)
            .combinedClickable(
                interactionSource = remember { MutableInteractionSource() }, indication = null,
                role = Role.Button, onClick = onClick, onLongClick = onLongClick,
            )
            .padding(start = KoanSpace.page, end = if (dragHandle == null) KoanSpace.page else 0.dp, top = if (art) 6.dp else 0.dp, bottom = if (art) 6.dp else 0.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(if (art) 10.dp else 6.dp),
    ) {
        Box(Modifier.widthIn(min = 16.dp), contentAlignment = Alignment.CenterEnd) {
            StatusMark(runtime, levels, item)
        }
        if (art) {
            item.sleeve()?.let { Sleeve(it, RowSize.sleeve) } ?: Box(Modifier.size(RowSize.sleeve))
        } else {
            KoanText(
                item.trackNumber?.toString() ?: "", KoanRole.Fine, KoanTone.Muted,
                modifier = Modifier.widthIn(min = 20.dp), data = true,
            )
        }
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(1.dp)) {
            KoanText(QueueFormat.title(item.title), KoanRole.Control, title, data = true, maxLines = 2)
            if (line.showArtist && item.artist.isNotEmpty()) {
                KoanText(
                    if (art && item.album.isNotEmpty()) "${item.artist} — ${item.album}" else item.artist,
                    KoanRole.Fine, KoanTone.Muted, data = true,
                )
            }
        }
        item.durationMs?.let {
            KoanText(
                QueueFormat.duration(it.toLong()), KoanRole.Fine, if (played) KoanTone.Muted else KoanTone.Ink,
                modifier = Modifier.widthIn(min = 40.dp), data = true,
            )
        }
        item.trackId?.let { id ->
            FavouriteMark(runtime, id, favourites)
        } ?: Box(Modifier.size(KoanSpaceHit))
        dragHandle?.invoke(this)
    }
    if (item.status == EntryStatus.DOWNLOADING) TransferLine(runtime, item.trackId, Modifier.align(Alignment.BottomStart))
    }
}

private val KoanSpaceHit = 32.dp
private const val SELECTED_ALPHA = 0.18f

/**
 * Where this track is and what it is doing, as one mark. Playing draws the
 * indicator (reading the playback state itself, so only the row playing
 * subscribes to it); queued, played, failed and waiting-to-download each have
 * their own.
 */
@Composable
private fun StatusMark(runtime: KoanRuntime, levels: PlayingLevels, item: QueueItem) {
    when (item.status) {
        EntryStatus.PLAYING -> {
            val playback by runtime.mirror.playback.collectAsStateWithLifecycle()
            PlayingBars(levels, live = playback.state == PlayState.PLAYING)
        }
        EntryStatus.PRIORITY_PENDING -> KoanIcon("download", tone = KoanTone.Accent, textSizeDp = 12f, description = "Queued for download")
        EntryStatus.FAILED -> KoanIcon("warning", tone = KoanTone.Bad, textSizeDp = 12f, description = item.failureReason ?: "Could not be fetched")
        EntryStatus.PLAYED -> KoanIcon("check", tone = KoanTone.Muted, textSizeDp = 12f)
        // A download shows in its own line along the row; the column stays so titles line up.
        EntryStatus.DOWNLOADING -> Box(Modifier.size(12.dp))
        EntryStatus.QUEUED -> KoanIcon("pending", tone = KoanTone.Rule, textSizeDp = 12f)
    }
}

/** The heart. Its own composable so a favourite toggled recomposes the hearts, not the rows. */
@Composable
private fun FavouriteMark(runtime: KoanRuntime, trackId: Long, favourites: State<Set<Long>>) {
    val on by remember(trackId, favourites) { derivedStateOf { trackId in favourites.value } }
    Box(
        Modifier
            .size(KoanSpaceHit)
            .clickable(
                interactionSource = remember { MutableInteractionSource() }, indication = null, role = Role.Button,
            ) { runtime.library.toggleFavouriteTrack(trackId) },
        contentAlignment = Alignment.Center,
    ) {
        KoanIcon(
            if (on) "heart-filled" else "heart", tone = if (on) KoanTone.Accent else KoanTone.Muted,
            textSizeDp = 13f, description = if (on) "Remove favourite" else "Favourite",
        )
    }
}

/**
 * A hairline of accent along the foot of a downloading row. The byte counts
 * move ten times a second, so they are collected here and read only while
 * drawing: the row does not recompose, and nor does anything above it.
 */
@Composable
private fun TransferLine(runtime: KoanRuntime, trackId: Long?, modifier: Modifier) {
    if (trackId == null) return
    val figures = runtime.mirror.figures.collectAsStateWithLifecycle()
    val colour = KoanTheme.colors.accent
    Box(
        modifier
            .fillMaxWidth()
            .height(2.dp)
            .drawBehind {
                val progress = figures.value[trackId]?.progress?.toFloat() ?: return@drawBehind
                drawRect(colour, Offset.Zero, Size(size.width * progress, size.height))
            },
    )
}
