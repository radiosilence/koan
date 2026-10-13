package cc.blit.koan.ui.pages

import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import cc.blit.koan.art.ArtworkSource
import cc.blit.koan.theme.KoanButton
import cc.blit.koan.theme.KoanButtonKind
import cc.blit.koan.theme.KoanIcon
import cc.blit.koan.theme.KoanIconButton
import cc.blit.koan.theme.KoanRole
import cc.blit.koan.theme.KoanRule
import cc.blit.koan.theme.KoanShape
import cc.blit.koan.theme.KoanSpace
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanTheme
import cc.blit.koan.theme.KoanTone
import cc.blit.koan.ui.Sleeve
import uniffi.koan_ffi.Track

/** The width the position column needs: as wide as the longest number, and no wider. */
fun numberColumn(count: Int): Dp = maxOf(20, count.toString().length * 9 + 4).dp

/**
 * What a row draws that can change under it, as plain values: the page reads
 * the player and the queue once and hands each row only its own answer, so a
 * track change recomposes the two rows it concerns and not the list.
 */
class RowState(
    val current: Boolean,
    val favourite: Boolean,
    val availability: Availability,
    val selecting: Boolean,
    val picked: Boolean,
    val highlighted: Boolean,
)

/**
 * One track of a record, or of an artist's appearances. Tap plays from here;
 * long-press starts a pick with this row ticked; in a pick a tap ticks. The
 * heart is always there, since a phone has no hover to reveal it.
 */
@Composable
fun TrackRowView(
    track: Track,
    position: Int,
    numberWidth: Dp,
    showsAlbum: Boolean,
    state: RowState,
    onTap: () -> Unit,
    onLongPress: () -> Unit,
    onHeart: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val colors = KoanTheme.colors
    val marked = state.picked || state.highlighted
    val credited = showsAlbum || track.artistName != track.albumArtistName
    val second = listOfNotNull(
        track.artistName.takeIf { credited },
        track.albumTitle.takeIf { showsAlbum },
        trackQuality(track),
    ).joinToString(" · ")
    Row(
        modifier
            .fillMaxWidth()
            .heightIn(min = if (showsAlbum) 60.dp else 52.dp)
            .background(if (marked) colors.surface else colors.bg)
            .combinedClickable(role = Role.Button, onClick = onTap, onLongClick = onLongPress)
            .semantics(mergeDescendants = true) {
                if (state.selecting) stateDescription = if (state.picked) "selected" else "not selected"
            },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.width(KoanShape.marker).fillMaxHeight().background(if (state.highlighted) colors.accent else colors.bg))
        Box(Modifier.width(KoanSpace.page - KoanShape.marker))
        Box(Modifier.width(numberWidth), contentAlignment = Alignment.CenterEnd) {
            when {
                state.selecting -> KoanIcon(if (state.picked) "selecting" else "select", tone = if (state.picked) KoanTone.Accent else KoanTone.Muted)
                state.current -> KoanIcon("play-mark", tone = KoanTone.Accent)
                else -> KoanText(position.toString(), KoanRole.Fine, KoanTone.Muted)
            }
        }
        Box(Modifier.width(KoanSpace.m))
        if (showsAlbum) {
            val albumId = track.albumId
            if (albumId != null) {
                Sleeve(ArtworkSource.Album(albumId), 44.dp)
            } else {
                Box(Modifier.size(44.dp), contentAlignment = Alignment.Center) { KoanIcon("track", tone = KoanTone.Muted) }
            }
            Box(Modifier.width(KoanSpace.m))
        }
        Column(Modifier.weight(1f).padding(vertical = KoanSpace.s)) {
            KoanText(
                displayTitle(track.title), KoanRole.Body,
                if (state.current) KoanTone.Accent else KoanTone.Ink, data = true, maxLines = 2,
            )
            if (second.isNotEmpty()) KoanText(second, KoanRole.Fine, KoanTone.Muted, data = true)
        }
        Box(Modifier.width(KoanSpace.s))
        AvailabilityMark(state.availability)
        KoanIconButton(
            icon = if (state.favourite) "heart-filled" else "heart",
            description = if (state.favourite) "Unfavourite ${track.title}" else "Favourite ${track.title}",
            onClick = onHeart,
            tone = if (state.favourite) KoanTone.Accent else KoanTone.Muted,
        )
        KoanText(formatDuration(track.durationMs), KoanRole.Fine, KoanTone.Muted, modifier = Modifier.padding(end = KoanSpace.page))
    }
}

/** Where a track's bytes are, as one fixed-size slot so a row does not shift as the mark changes. */
@Composable
fun AvailabilityMark(availability: Availability) {
    val (glyph, tone, label) = when (availability) {
        Availability.Waiting -> Triple("pending", KoanTone.Muted, "Queued for download")
        Availability.Downloading -> Triple("download", KoanTone.Accent, "Downloading")
        Availability.Failed -> Triple("warning", KoanTone.Bad, "Could not be fetched")
        Availability.Kept -> Triple("cloud-kept", KoanTone.Ink, "On your server, downloaded")
        Availability.Cloud -> Triple("cloud", KoanTone.Muted, "On your server, downloads on play")
        Availability.Local -> Triple("drive", KoanTone.Ink, "File on this device")
        Availability.None -> Triple(null, KoanTone.Muted, null)
    }
    Box(Modifier.size(20.dp), contentAlignment = Alignment.Center) {
        if (glyph != null) KoanIcon(glyph, tone = tone, textSizeDp = 12f, description = label)
    }
}

/** A disc's heading in a record on more than one. */
@Composable
fun DiscHeading(disc: Int, modifier: Modifier = Modifier) {
    Column(modifier.fillMaxWidth().padding(top = KoanSpace.l)) {
        KoanText("Disc $disc", KoanRole.Meta, KoanTone.Muted, modifier = Modifier.padding(horizontal = KoanSpace.page, vertical = KoanSpace.s))
        KoanRule(Modifier.padding(horizontal = KoanSpace.page))
    }
}

/** One thing a [SelectionBar] can do. */
class BarAction(val label: String, val onClick: () -> Unit, val enabled: Boolean = true)

/**
 * The bar a pick shows along the foot of the page: how many are ticked and
 * what to do with them. Playing or queueing puts the page back to normal; the
 * page does that in the action it passes.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun SelectionBar(count: Int, actions: List<BarAction>, modifier: Modifier = Modifier) {
    Column(modifier.fillMaxWidth().background(KoanTheme.colors.bg)) {
        KoanRule()
        KoanText(
            if (count == 0) "Pick something" else "$count selected", KoanRole.Meta, KoanTone.Muted,
            modifier = Modifier.padding(horizontal = KoanSpace.page, vertical = KoanSpace.s),
        )
        FlowRow(
            Modifier.fillMaxWidth().padding(horizontal = KoanSpace.m).padding(bottom = KoanSpace.s),
            horizontalArrangement = Arrangement.spacedBy(KoanSpace.xs),
        ) {
            for (action in actions) {
                KoanButton(action.label, action.onClick, kind = KoanButtonKind.Compact, enabled = action.enabled)
            }
        }
    }
}

/** A page with nothing to show but a sentence, and the ways out of it. */
@Composable
fun RecordMessage(text: String, actions: List<BarAction>, modifier: Modifier = Modifier) {
    Column(
        modifier.fillMaxSize().padding(KoanSpace.page),
        verticalArrangement = Arrangement.spacedBy(KoanSpace.l, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        KoanText(text, KoanRole.Body, KoanTone.Muted, maxLines = 3)
        Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.s)) {
            for (action in actions) KoanButton(action.label, action.onClick, kind = KoanButtonKind.Bordered)
        }
    }
}
