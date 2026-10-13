package cc.blit.koan.ui.pages

import uniffi.koan_ffi.Transfer
import uniffi.koan_ffi.TransferFigure
import uniffi.koan_ffi.TransferState

/**
 * What a download row says, from `DownloadsView.swift`'s `DownloadRow`. Pure,
 * so it is tested without a screen. The figure is the transfer's byte counts
 * and is read only while the transfer is queued or running: a settled row does
 * not follow the fast stream.
 */
object TransferText {
    fun isLive(state: TransferState): Boolean = state == TransferState.RUNNING || state == TransferState.QUEUED

    /** What has arrived, 0 to 1. Whole once it has landed; empty, not full, when the server gave no length. */
    fun fraction(state: TransferState, figure: TransferFigure?): Float = when (state) {
        TransferState.DONE -> 1f
        else -> (figure?.progress ?: 0.0).toFloat().coerceIn(0f, 1f)
    }

    /** The short word at the row's trailing edge. */
    fun figure(transfer: Transfer, figure: TransferFigure?): String = when (transfer.state) {
        TransferState.DONE -> "Done"
        TransferState.FAILED -> "Failed"
        TransferState.QUEUED -> "Queued"
        TransferState.RUNNING -> figure?.progress?.let { "${(it * 100).toInt().coerceIn(0, 100)}%" }.orEmpty()
    }

    /** "Artist, 4.2 MB/s, 210 MB of 451 MB", dropping what is not known. A rate of nothing is a stall, and saying so is the point. */
    fun subtitle(transfer: Transfer, figure: TransferFigure?): String = when (transfer.state) {
        TransferState.FAILED -> transfer.failureReason ?: "Couldn't be fetched"
        TransferState.QUEUED -> if (transfer.artist.isEmpty()) "Waiting" else "${transfer.artist} — waiting"
        TransferState.DONE -> if (transfer.artist.isEmpty()) "Downloaded" else "${transfer.artist} — downloaded"
        TransferState.RUNNING -> {
            val parts = ArrayList<String>(4)
            if (transfer.artist.isNotEmpty()) parts += transfer.artist
            val rate = figure?.bytesPerSecond ?: 0uL
            parts += if (rate > 0uL) "${bytes(rate.toLong())}/s" else "stalled"
            val written = figure?.bytesWritten ?: 0uL
            val total = figure?.totalBytes ?: 0uL
            if (total > 0uL) parts += "${bytes(written.toLong())} of ${bytes(total.toLong())}"
            else if (written > 0uL) parts += bytes(written.toLong())
            parts.joinToString(" · ")
        }
    }

    /** Decimal units, as a file size is shown: "0 B", "812 KB", "4.2 MB". */
    fun bytes(count: Long): String {
        if (count < 1000) return "$count B"
        val units = arrayOf("KB", "MB", "GB", "TB")
        var value = count / 1000.0
        var unit = 0
        while (value >= 1000 && unit < units.lastIndex) {
            value /= 1000
            unit++
        }
        return if (value < 10) "%.1f %s".format(java.util.Locale.ROOT, value, units[unit])
        else "%.0f %s".format(java.util.Locale.ROOT, value, units[unit])
    }

    /** "12,400 of 49,700 tracks" where the task says what it counts, "12,345 / 48,087" where it does not. */
    fun counts(done: Long?, total: Long?, unit: String?): String? {
        done ?: return null
        val u = unit?.let { " $it" }.orEmpty()
        val nf = java.text.NumberFormat.getIntegerInstance(java.util.Locale.ROOT)
        return if (total != null && total > 0) {
            val sep = if (unit == null) " / " else " of "
            "${nf.format(done)}$sep${nf.format(total)}$u"
        } else {
            "${nf.format(done)}$u"
        }
    }
}
