package cc.blit.koan.ui.library

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.engine.EngineMirror
import cc.blit.koan.ffi.Track
import cc.blit.koan.ffi.Transfer
import cc.blit.koan.ffi.TransferState
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn

/** Where a track's bytes are, as one mark. */
enum class Availability {
    /** A file of the library's own, not on the server. */
    Local,

    /** On the server, and downloaded here. */
    Kept,

    /** On the server only: it downloads when it plays. */
    ServerOnly,

    /** Waiting its turn to download. */
    Queued,

    /** Arriving. */
    Downloading,

    /** The download failed. */
    Failed,

    /** Nowhere this device can reach. */
    None,
}

/**
 * What a track row shows: the download store's word on a transfer while
 * there is one, otherwise the library row's. The library row learns a
 * download finished only when the library is read again, so a settled
 * transfer says kept in the meantime.
 */
fun availability(onServer: Boolean, onDisk: Boolean, transfer: TransferState?) = when (transfer) {
    TransferState.QUEUED -> Availability.Queued
    TransferState.RUNNING -> Availability.Downloading
    TransferState.FAILED -> Availability.Failed
    TransferState.DONE -> if (onServer) Availability.Kept else Availability.Local
    null -> when {
        onServer && onDisk -> Availability.Kept
        onServer -> Availability.ServerOnly
        onDisk -> Availability.Local
        else -> Availability.None
    }
}

/** The download store by track, one map for every row on screen. */
private object TransfersByTrack {
    private var held: StateFlow<Map<Long, Transfer>>? = null

    fun of(mirror: EngineMirror): StateFlow<Map<Long, Transfer>> = synchronized(this) {
        held ?: mirror.transfers
            .map { all -> all.associateBy { it.trackId } }
            .stateIn(CoroutineScope(SupervisorJob() + Dispatchers.Default), SharingStarted.Eagerly, emptyMap())
            .also { held = it }
    }
}

/**
 * Where a track's bytes are, in one fixed slot: a cloud for the server, filled
 * once it is here; a ring filling as it arrives; a warning when it could not
 * be fetched; the device mark for a local file.
 *
 * Each row follows its own track's transfer, so a transfer changing state
 * recomposes its row and no other, and only a running transfer reads the byte
 * counts, which move many times a second.
 */
@Composable
fun TrackAvailability(track: Track, modifier: Modifier = Modifier) {
    val mirror = koanApp().mirror
    val transfers = TransfersByTrack.of(mirror)
    val transfer by remember(track.id, transfers) {
        transfers.map { it[track.id] }.distinctUntilChanged()
    }.collectAsStateWithLifecycle(null)
    val muted = KoanTheme.color(KoanTone.Muted)
    Box(modifier.size(18.dp), contentAlignment = Alignment.Center) {
        when (availability(track.onServer, track.onDisk, transfer?.state)) {
            Availability.Downloading -> DownloadRing(track.id, mirror)
            Availability.Queued -> KoanIcon(KoanIcons.downloads, "Queued for download", size = 12.sp, color = KoanTheme.color(KoanTone.Accent))
            Availability.Failed ->
                KoanIcon(KoanIcons.warningFilled, transfer?.failureReason ?: "Couldn't be fetched", size = 12.sp, color = KoanTheme.color(KoanTone.Bad))
            Availability.Kept -> KoanIcon(KoanIcons.cloudKept, "On your server, downloaded", size = 12.sp, color = muted)
            Availability.ServerOnly -> KoanIcon(KoanIcons.cloud, "On your server, downloads on play", size = 12.sp, color = muted)
            Availability.Local -> KoanIcon(KoanIcons.onDevice, "Local file", size = 12.sp, color = muted)
            Availability.None -> Unit
        }
    }
}

/** A ring that fills as the bytes land, or spins when the server gave no length. */
@Composable
private fun DownloadRing(trackId: Long, mirror: EngineMirror) {
    val progress by remember(trackId) {
        mirror.figures.map { all -> all.firstOrNull { it.trackId == trackId }?.progress }.distinctUntilChanged()
    }.collectAsStateWithLifecycle(null)
    val colour = KoanTheme.look.accent
    val fraction = progress
    if (fraction != null) {
        CircularProgressIndicator(progress = { fraction.toFloat().coerceAtLeast(0.02f) }, Modifier.size(12.dp), color = colour, strokeWidth = 1.5.dp)
    } else {
        CircularProgressIndicator(Modifier.size(12.dp), color = colour, strokeWidth = 1.5.dp)
    }
}
