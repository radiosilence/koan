package cc.blit.koan.ui.playlists

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.AlertDialog
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import cc.blit.koan.art.Art
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanField
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import cc.blit.koan.ui.theme.LocalKoanTokens
import cc.blit.koan.ui.theme.koanRow
import coil3.compose.AsyncImage

/**
 * A row of one of these pages: a sleeve or a mark, a title over a line of
 * detail, and something at its end. Tapped to open or play, held for its menu.
 * No ripple and no rule between rows; a selected row takes the `surface` ground.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun CollectionRow(
    title: String,
    detail: String?,
    modifier: Modifier = Modifier,
    titleTone: KoanTone = KoanTone.Ink,
    appText: Boolean = false,
    detailTone: KoanTone = KoanTone.Muted,
    selected: Boolean = false,
    onClick: (() -> Unit)? = null,
    onLongClick: (() -> Unit)? = null,
    leading: (@Composable () -> Unit)? = null,
    below: (@Composable () -> Unit)? = null,
    trailing: (@Composable () -> Unit)? = null,
) {
    Row(
        modifier
            .fillMaxWidth()
            .koanRow(selected)
            .combinedClickable(
                interactionSource = remember { MutableInteractionSource() },
                indication = null,
                enabled = onClick != null || onLongClick != null,
                onClick = onClick ?: {},
                onLongClick = onLongClick,
            )
            .padding(horizontal = LocalKoanTokens.current.gutter, vertical = KoanSpace.s),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(KoanSpace.m),
    ) {
        leading?.invoke()
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            KoanText(title, tone = titleTone, appText = appText, maxLines = 1)
            detail?.let { KoanText(it, role = KoanType.Meta, tone = detailTone, maxLines = 1) }
            below?.invoke()
        }
        trailing?.invoke()
    }
}

/** How far something has got: the whole length in `rule`, what is done in `ink`. */
@Composable
fun KoanBar(fraction: Float, modifier: Modifier = Modifier) {
    Box(modifier.fillMaxWidth().height(3.dp).background(KoanTheme.color(KoanTone.Rule))) {
        Box(Modifier.fillMaxWidth(fraction.coerceIn(0f, 1f)).fillMaxHeight().background(KoanTheme.color(KoanTone.Ink)))
    }
}

/**
 * A playlist's face: the covers of up to four of its records, two by two, or
 * the one cover when it holds fewer than four.
 */
@Composable
fun PlaylistMosaic(albumIds: List<Long>, size: Dp, modifier: Modifier = Modifier) {
    val box = modifier.size(size)
    when {
        albumIds.size >= 4 -> Column(box) {
            albumIds.take(4).chunked(2).forEach { pair ->
                Row(Modifier.weight(1f)) {
                    pair.forEach { id ->
                        AsyncImage(Art.album(id), null, Modifier.weight(1f).aspectRatio(1f), contentScale = ContentScale.Crop)
                    }
                }
            }
        }
        albumIds.isNotEmpty() -> AsyncImage(Art.album(albumIds.first()), null, box, contentScale = ContentScale.Crop)
        else -> Box(box.background(KoanTheme.color(KoanTone.Rule)), contentAlignment = Alignment.Center) {
            KoanIcon(KoanIcons.playlist, null, color = KoanTheme.color(KoanTone.Muted))
        }
    }
}

/** Ask for a playlist's name, for a new one or a rename. */
@Composable
fun NameDialog(title: String, confirm: String, initial: String = "", onDismiss: () -> Unit, onName: (String) -> Unit) {
    var name by remember { mutableStateOf(initial) }
    val focus = remember { FocusRequester() }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { KoanText(title, role = KoanType.TitleSmall, appText = true) },
        text = { KoanField(name, { name = it }, Modifier.fillMaxWidth().focusRequester(focus), placeholder = "Name") },
        confirmButton = { KoanButton(confirm, { onName(name.trim()) }, enabled = name.isNotBlank()) },
        dismissButton = { KoanButton("Cancel", onDismiss) },
    )
    LaunchedEffect(Unit) { focus.requestFocus() }
}

/** Ask before something that cannot be undone. */
@Composable
fun ConfirmDialog(title: String, detail: String, confirm: String, onDismiss: () -> Unit, onConfirm: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { KoanText(title, role = KoanType.TitleSmall, appText = true) },
        text = { KoanText(detail, role = KoanType.Meta, tone = KoanTone.Muted) },
        confirmButton = { KoanButton(confirm, { onDismiss(); onConfirm() }, destructive = true) },
        dismissButton = { KoanButton("Cancel", onDismiss) },
    )
}

/** "48 min", or "2 h 5 min": a playlist's length, to the minute. */
fun duration(ms: Long): String {
    val minutes = (ms + 30_000) / 60_000
    return if (minutes < 60) "$minutes min" else "${minutes / 60} h ${minutes % 60} min"
}
