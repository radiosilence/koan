package cc.blit.koan.ui.pages

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.art.ArtworkSource
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.state.ActivityTask
import cc.blit.koan.theme.KoanButton
import cc.blit.koan.theme.KoanButtonKind
import cc.blit.koan.theme.KoanIconButton
import cc.blit.koan.theme.KoanRole
import cc.blit.koan.theme.KoanRule
import cc.blit.koan.theme.KoanSpace
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanTheme
import cc.blit.koan.theme.KoanTone
import cc.blit.koan.ui.Sleeve
import kotlinx.coroutines.flow.StateFlow
import uniffi.koan_ffi.Transfer
import uniffi.koan_ffi.TransferFigure
import uniffi.koan_ffi.TransferState

/**
 * What koan is fetching from the server, what it just finished fetching, and
 * what else it is busy with (Section.Downloads). The port of
 * `DownloadsView.swift`, with the sidebar's running-task rows
 * (`ActivityIndicator.swift`) at the top, since a phone has no sidebar.
 *
 * The first place to look when something is not playing: a transfer that has
 * stopped moving says "stalled" and a failure keeps its reason.
 *
 * This page collects the structural slices only (`transfers`). The byte
 * counts, which move ten times a second, are passed down as a flow and
 * collected by the bar and the two lines of text that draw them; see
 * `TransferMeter.kt`. The activity block collects its own tasks the same way.
 */
@Composable
fun DownloadsPage(runtime: KoanRuntime) {
    val transfers by runtime.mirror.transfers.collectAsStateWithLifecycle()
    val navigator = runtime.navigator
    val library = runtime.library
    val activity = runtime.activity
    DownloadsContent(
        transfers = transfers,
        figures = runtime.mirror.figures,
        tasks = activity.tasks,
        onShowInLibrary = { t -> t.albumId?.let { navigator.openAlbum(it, highlighting = t.trackId) } },
        onRemoveFile = { library.clearDownloads(listOf(it)) },
        onCancelTask = activity::cancel,
    )
}

/** The page over plain values and flows, so it composes in a test with no runtime behind it. */
@Composable
internal fun DownloadsContent(
    transfers: List<Transfer>,
    figures: StateFlow<Map<Long, TransferFigure>>,
    tasks: StateFlow<List<ActivityTask>>,
    onShowInLibrary: (Transfer) -> Unit,
    onRemoveFile: (Long) -> Unit,
    onCancelTask: (String) -> Unit,
) {
    LazyColumn(Modifier.fillMaxSize(), contentPadding = androidx.compose.foundation.layout.PaddingValues(bottom = KoanSpace.l)) {
        item(key = "header", contentType = "header") {
            Column(Modifier.padding(KoanSpace.page), verticalArrangement = Arrangement.spacedBy(KoanSpace.xs)) {
                KoanText("Downloads", KoanRole.Title, KoanTone.Strong)
                if (transfers.isNotEmpty()) {
                    val moving = transfers.count { !it.state.isSettledState() }
                    KoanText(
                        if (moving == 0) "Nothing in flight" else "$moving in flight",
                        KoanRole.Control, KoanTone.Muted,
                    )
                }
            }
        }
        item(key = "activity", contentType = "activity") {
            ActivityBlock(tasks, onCancelTask)
        }
        if (transfers.isEmpty()) {
            item(key = "empty", contentType = "empty") {
                Column(Modifier.padding(KoanSpace.page), verticalArrangement = Arrangement.spacedBy(KoanSpace.xs)) {
                    KoanText("Nothing downloading", KoanRole.Body, KoanTone.Strong)
                    KoanText("Tracks fetched from your server appear here while they arrive.", KoanRole.Meta, KoanTone.Muted, maxLines = 3)
                }
            }
        } else {
            items(transfers, key = { it.trackId }, contentType = { "transfer" }) { transfer ->
                DownloadRow(transfer, figures, onShowInLibrary, onRemoveFile)
            }
        }
    }
}

private fun TransferState.isSettledState() = this == TransferState.DONE || this == TransferState.FAILED

@Composable
private fun DownloadRow(
    transfer: Transfer,
    figures: StateFlow<Map<Long, TransferFigure>>,
    onShowInLibrary: (Transfer) -> Unit,
    onRemoveFile: (Long) -> Unit,
) {
    // The words come from the fast stream but are recomposed only when they differ.
    val figureText by rememberTransferText(transfer, figures, TransferText::figure)
    val subtitle by rememberTransferText(transfer, figures, TransferText::subtitle)
    Column {
        Row(
            Modifier.fillMaxWidth().padding(horizontal = KoanSpace.page, vertical = KoanSpace.s),
            horizontalArrangement = Arrangement.spacedBy(KoanSpace.m),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            // A record is what you recognise a download by.
            Sleeve(ArtworkSource.Track(transfer.trackId), 48.dp)
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(KoanSpace.xs)) {
                Row(verticalAlignment = Alignment.Bottom) {
                    KoanText(transfer.title, KoanRole.Body, KoanTone.Ink, data = true, modifier = Modifier.weight(1f))
                    KoanText(figureText, KoanRole.Fine, KoanTone.Muted, data = true)
                }
                TransferBar(transfer, figures)
                KoanText(
                    subtitle, KoanRole.Fine,
                    if (transfer.state == TransferState.FAILED) KoanTone.Bad else KoanTone.Muted,
                    data = true,
                )
                Row {
                    if (transfer.albumId != null) {
                        KoanButton("show in library", { onShowInLibrary(transfer) }, kind = KoanButtonKind.Link)
                    }
                    if (transfer.state == TransferState.DONE) {
                        KoanButton("remove file", { onRemoveFile(transfer.trackId) }, kind = KoanButtonKind.Text)
                    }
                }
            }
        }
        KoanRule()
    }
}

/** The running tasks (a scan, a sync, a big queue add), or nothing at all when idle: a permanent empty state is furniture. */
@Composable
private fun ActivityBlock(tasks: StateFlow<List<ActivityTask>>, onCancel: (String) -> Unit) {
    val running by tasks.collectAsStateWithLifecycle()
    if (running.isEmpty()) return
    Column(
        Modifier.fillMaxWidth().padding(horizontal = KoanSpace.page, vertical = KoanSpace.s)
            .background(KoanTheme.colors.surface).padding(KoanSpace.m),
        verticalArrangement = Arrangement.spacedBy(KoanSpace.m),
    ) {
        for (task in running) key(task.id) { ActivityRow(task, onCancel) }
    }
}

@Composable
private fun ActivityRow(task: ActivityTask, onCancel: (String) -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(KoanSpace.xs)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            KoanText(task.label, KoanRole.Control, KoanTone.Ink, modifier = Modifier.weight(1f))
            TransferText.counts(task.done, task.total, task.unit)?.let { KoanText(it, KoanRole.Fine, KoanTone.Muted, data = true) }
        }
        // Determinate where the engine can say, empty where it cannot; same height, so the row does not jump when a total arrives.
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(KoanSpace.s)) {
            val progress = task.progress?.toFloat() ?: 0f
            FractionBar(Modifier.weight(1f)) { progress }
            // Only where stopping does something. A button that cannot cancel is worse than none.
            if (task.cancellable) {
                KoanIconButton("close", "Stop ${task.label}", { onCancel(task.id) })
            }
        }
        task.detail?.let { KoanText(it, KoanRole.Fine, KoanTone.Muted, data = true) }
    }
}
