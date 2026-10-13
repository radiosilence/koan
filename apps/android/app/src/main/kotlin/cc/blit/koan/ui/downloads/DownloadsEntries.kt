package cc.blit.koan.ui.downloads

import android.text.format.Formatter
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.R
import cc.blit.koan.art.Art
import cc.blit.koan.engine.EngineMirror
import cc.blit.koan.ffi.Album
import cc.blit.koan.ffi.AlbumSort
import cc.blit.koan.ffi.KoanEngine
import cc.blit.koan.ffi.Transfer
import cc.blit.koan.ffi.TransferState
import cc.blit.koan.ui.StubPage
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.library.AlbumRoute
import cc.blit.koan.ui.library.NoFilter
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.navigation.nonce
import cc.blit.koan.ui.playlists.ConfirmDialog
import cc.blit.koan.ui.playlists.PageBar
import cc.blit.koan.ui.theme.LocalKoanTokens
import coil3.compose.AsyncImage
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

/** What is on this device: the records with files here, and how much of each. */
@Serializable
data class DownloadedRoute(val nonce: String = nonce()) : NavKey

fun EntryProviderScope<NavKey>.downloadsEntries() {
    entry<DownloadsRoute> { DownloadsPage() }
    entry<DownloadedRoute> { DownloadedPage() }
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
                TextButton(onClick = { app.mirror.launch { clearSettledDownloads() } }) { Text("Clear Finished") }
            }
        }
        LazyColumn {
            item(key = "cache") { CacheUsage(onRemoveAll = { clearing = true }) }
            if (transfers.isEmpty()) {
                item(key = "empty") { StubPage("Nothing downloading", Modifier.height(160.dp)) }
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
    val gutter = LocalKoanTokens.current.gutter
    val size = { bytes: Long -> Formatter.formatShortFileSize(context, bytes) }

    Column(Modifier.padding(horizontal = gutter, vertical = LocalKoanTokens.current.rowGap)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                limit?.let { "${size(used)} of ${size(it)}" } ?: "${size(used)} on this device",
                style = MaterialTheme.typography.titleMedium,
                modifier = Modifier.weight(1f),
            )
            TextButton(onClick = onRemoveAll, enabled = used > 0) { Text("Remove All") }
        }
        limit?.takeIf { it > 0 }?.let {
            LinearProgressIndicator(progress = { (used.toFloat() / it).coerceIn(0f, 1f) }, modifier = Modifier.fillMaxWidth())
            Spacer(Modifier.height(4.dp))
        }
    }
    ListItem(
        headlineContent = { Text("Downloaded Records") },
        supportingContent = { Text("What can play here without your server") },
        leadingContent = { Icon(painterResource(R.drawable.koan_cloud_kept), null) },
        modifier = Modifier.clickable { nav.push(DownloadedRoute()) },
    )
    HorizontalDivider()
}

@OptIn(ExperimentalFoundationApi::class)
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

    val subtitle = when (transfer.state) {
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
    val fraction = when (transfer.state) {
        TransferState.DONE -> 1f
        else -> figure?.progress?.toFloat() ?: 0f
    }

    Box(modifier) {
        ListItem(
            headlineContent = { Text(transfer.title, maxLines = 1, overflow = TextOverflow.Ellipsis) },
            supportingContent = {
                Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    LinearProgressIndicator(progress = { fraction }, modifier = Modifier.fillMaxWidth().padding(top = 4.dp))
                    Text(
                        subtitle,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        color = if (transfer.state == TransferState.FAILED) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            },
            leadingContent = { AsyncImage(transfer.albumId?.let { Art.album(it) } ?: Art.track(transfer.trackId), null, Modifier.size(LocalKoanTokens.current.thumb)) },
            trailingContent = {
                Text(
                    when (transfer.state) {
                        TransferState.DONE -> "Done"
                        TransferState.FAILED -> "Failed"
                        TransferState.QUEUED -> "Queued"
                        TransferState.RUNNING -> figure?.progress?.let { "${(it * 100).toInt()}%" }.orEmpty()
                    },
                    style = MaterialTheme.typography.labelMedium,
                )
            },
            modifier = Modifier.combinedClickable(onClick = { transfer.albumId?.let { nav.push(AlbumRoute(it)) } }, onLongClick = { menu = true }),
        )
        DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
            transfer.albumId?.let { albumId ->
                DropdownMenuItem(
                    text = { Text("Show in Library") },
                    leadingIcon = { Icon(painterResource(R.drawable.koan_album), null) },
                    onClick = { menu = false; nav.push(AlbumRoute(albumId)) },
                )
            }
            if (transfer.state == TransferState.DONE) {
                DownloadMenuItems(onDisk = true, dismiss = { menu = false }) { listOf(transfer.trackId) }
            }
        }
    }
}

/** The records with files on this device, fully there first, asked for again as the library moves. */
class DownloadedModel(mirror: EngineMirror) : ViewModel() {
    val albums: StateFlow<List<Album>?> = mirror.library
        .mapLatest {
            runCatching { mirror.engine().albums(null, AlbumSort.DOWNLOADED, 0, null, NoFilter.copy(downloaded = true)) }
                .getOrDefault(emptyList())
        }
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)
}

/** What a phone can play with no signal: each record with a bar for how much of it is here. */
@Composable
private fun DownloadedPage() {
    val app = koanApp()
    val model = viewModel { DownloadedModel(app.mirror) }
    val albums by model.albums.collectAsStateWithLifecycle()
    Column {
        PageBar("Downloaded")
        val shown = albums
        when {
            shown == null -> Unit
            shown.isEmpty() -> StubPage("Nothing downloaded yet")
            else -> LazyColumn {
                items(shown, key = { it.id }) { DownloadedRow(it, Modifier.animateItem()) }
            }
        }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun DownloadedRow(album: Album, modifier: Modifier) {
    val nav = LocalNavigator.current
    var menu by remember { mutableStateOf(false) }
    val have = album.onDevice?.have ?: 0u
    val total = album.onDevice?.total ?: 0u
    val albumTracks: suspend KoanEngine.() -> List<Long> = { trackIds(album.id, null) }

    Box(modifier) {
        ListItem(
            headlineContent = { Text(album.title, maxLines = 1, overflow = TextOverflow.Ellipsis) },
            supportingContent = {
                Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text(album.artistName, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    if (total > 0u && have < total) {
                        LinearProgressIndicator(progress = { have.toFloat() / total.toFloat() }, modifier = Modifier.fillMaxWidth())
                    }
                }
            },
            leadingContent = { AsyncImage(Art.album(album.id), null, Modifier.size(LocalKoanTokens.current.thumb)) },
            trailingContent = {
                Text(if (have == total) "All" else "$have of $total", style = MaterialTheme.typography.labelMedium)
            },
            modifier = Modifier.combinedClickable(onClick = { nav.push(AlbumRoute(album.id)) }, onLongClick = { menu = true }),
        )
        DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
            DownloadMenuItems(onDisk = if (have == total) true else null, dismiss = { menu = false }, trackIds = albumTracks)
        }
    }
}
