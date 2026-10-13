package cc.blit.koan.ui.downloads

import android.text.format.Formatter
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.DropdownMenu
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.art.Art
import cc.blit.koan.engine.EngineMirror
import cc.blit.koan.ffi.Album
import cc.blit.koan.ffi.AlbumSort
import cc.blit.koan.ffi.KoanEngine
import cc.blit.koan.ffi.Transfer
import cc.blit.koan.ffi.TransferState
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.library.AlbumRoute
import cc.blit.koan.ui.library.Shelf
import cc.blit.koan.ui.library.ShelfRoute
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.navigation.nonce
import cc.blit.koan.ui.playlists.CollectionRow
import cc.blit.koan.ui.playlists.ConfirmDialog
import cc.blit.koan.ui.playlists.KoanBar
import cc.blit.koan.ui.playlists.MenuItem
import cc.blit.koan.ui.playlists.PageBar
import cc.blit.koan.ui.playlists.Sleeve
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import cc.blit.koan.ui.theme.KoanUnavailable
import cc.blit.koan.ui.theme.LocalKoanTokens
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.mapLatest
import kotlinx.coroutines.flow.stateIn
import kotlinx.serialization.Serializable

/** What is arriving from the server, and what just did. */
@Serializable
data class DownloadsRoute(val nonce: String = nonce()) : NavKey

fun EntryProviderScope<NavKey>.downloadsEntries() {
    entry<DownloadsRoute> { DownloadsPage() }
}

/**
 * The transfers, and the cache they fill against its limit. The first place to
 * look when something is not playing: a transfer that has stopped moving says
 * so, and a failure keeps its reason.
 */
@Composable
private fun DownloadsPage() {
    val app = koanApp()
    val transfers by app.mirror.transfers.collectAsStateWithLifecycle()
    var clearing by remember { mutableStateOf(false) }

    Column {
        PageBar("Downloads") {
            if (transfers.any { it.state == TransferState.DONE || it.state == TransferState.FAILED }) {
                KoanButton("Clear Finished", { app.mirror.launch { clearSettledDownloads() } }, kind = KoanButtonKind.Text)
            }
        }
        LazyColumn {
            item(key = "cache") { CacheUsage(onRemoveAll = { clearing = true }) }
            if (transfers.isEmpty()) {
                item(key = "empty") {
                    KoanUnavailable(
                        "Nothing downloading",
                        KoanIcons.downloads,
                        "Tracks fetched from your server appear here while they arrive.",
                        Modifier.height(240.dp),
                    )
                }
            }
            items(transfers, key = { it.trackId }) { TransferRow(it, Modifier.animateItem()) }
        }
    }

    if (clearing) {
        ConfirmDialog(
            "Remove All Downloads?",
            "Every file fetched from your server is deleted from this device. The tracks stay in the library and download again when played.",
            "Remove",
            onDismiss = { clearing = false },
        ) { app.mirror.launch { clearDownloadCache() } }
    }
}

/** How much the downloads take up, against the limit when one is set, and the way to what they are. */
@Composable
private fun CacheUsage(onRemoveAll: () -> Unit) {
    val app = koanApp()
    val context = LocalContext.current
    val nav = LocalNavigator.current
    val used by app.mirror.cacheBytes.collectAsStateWithLifecycle()
    // Read again as the cache moves; a limit changed in Settings shows on the next visit.
    val limit by produceState<Long?>(null, used) {
        value = runCatching { app.mirror.engine().cacheLimitBytes()?.toLong() }.getOrNull()
    }
    val size = { bytes: Long -> Formatter.formatShortFileSize(context, bytes) }

    Column(
        Modifier.padding(horizontal = LocalKoanTokens.current.gutter, vertical = KoanSpace.s),
        verticalArrangement = Arrangement.spacedBy(KoanSpace.s),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            KoanText(
                limit?.let { "${size(used)} of ${size(it)}" } ?: "${size(used)} on this device",
                Modifier.weight(1f),
            )
            KoanButton("Remove All", onRemoveAll, kind = KoanButtonKind.Compact, enabled = used > 0, destructive = true)
        }
        limit?.takeIf { it > 0 }?.let { KoanBar(used.toFloat() / it) }
    }
    CollectionRow(
        "Downloaded Records",
        "What plays here without your server",
        appText = true,
        onClick = { nav.push(ShelfRoute(Shelf.Downloaded)) },
        leading = { KoanIcon(KoanIcons.onDevice, null) },
        trailing = { KoanIcon(KoanIcons.disclosure, null, color = KoanTheme.color(KoanTone.Muted)) },
    )
}

@Composable
private fun TransferRow(transfer: Transfer, modifier: Modifier) {
    val app = koanApp()
    val context = LocalContext.current
    val nav = LocalNavigator.current
    val running = transfer.state == TransferState.RUNNING || transfer.state == TransferState.QUEUED
    // Only this transfer's figure, so a row redraws at its own rate and a settled one not at all.
    val figure by remember(transfer.trackId, running) {
        app.mirror.figures.map { all -> if (running) all.firstOrNull { it.trackId == transfer.trackId } else null }.distinctUntilChanged()
    }.collectAsStateWithLifecycle(null)
    var menu by remember { mutableStateOf(false) }
    val size = { bytes: ULong -> Formatter.formatShortFileSize(context, bytes.toLong()) }

    val detail = when (transfer.state) {
        TransferState.FAILED -> transfer.failureReason ?: "Couldn't be fetched"
        TransferState.QUEUED -> listOf(transfer.artist, "waiting").filter { it.isNotEmpty() }.joinToString(" — ")
        TransferState.DONE -> listOf(transfer.artist, "downloaded").filter { it.isNotEmpty() }.joinToString(" — ")
        TransferState.RUNNING -> buildList {
            if (transfer.artist.isNotEmpty()) add(transfer.artist)
            val f = figure
            // A rate of nothing is a stall, and saying so is the point of the row.
            add(if (f != null && f.bytesPerSecond > 0u) "${size(f.bytesPerSecond)}/s" else "stalled")
            if (f != null && f.totalBytes > 0u) add("${size(f.bytesWritten)} of ${size(f.totalBytes)}")
        }.joinToString(" · ")
    }
    // Whole once it has landed, whatever the last figure said; empty while the length is unknown.
    val fraction = if (transfer.state == TransferState.DONE) 1f else figure?.progress?.toFloat() ?: 0f

    Box(modifier) {
        CollectionRow(
            transfer.title,
            detail,
            detailTone = if (transfer.state == TransferState.FAILED) KoanTone.Bad else KoanTone.Muted,
            onClick = transfer.albumId?.let { id -> { nav.push(AlbumRoute(id)) } },
            onLongClick = { menu = true },
            leading = { Sleeve(transfer.albumId?.let { Art.album(it) } ?: Art.track(transfer.trackId)) },
            below = { KoanBar(fraction, Modifier.padding(top = KoanSpace.xs)) },
            trailing = {
                KoanText(
                    when (transfer.state) {
                        TransferState.DONE -> "done"
                        TransferState.FAILED -> "failed"
                        TransferState.QUEUED -> "queued"
                        TransferState.RUNNING -> figure?.progress?.let { "${(it * 100).toInt()}%" }.orEmpty()
                    },
                    role = KoanType.Meta,
                    tone = KoanTone.Muted,
                    appText = true,
                )
            },
        )
        DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
            transfer.albumId?.let { albumId ->
                MenuItem("Show in Library", KoanIcons.album) { menu = false; nav.push(AlbumRoute(albumId, transfer.trackId)) }
            }
            if (transfer.state == TransferState.DONE) {
                DownloadMenuItems(onDisk = true, dismiss = { menu = false }) { listOf(transfer.trackId) }
            }
        }
    }
}
