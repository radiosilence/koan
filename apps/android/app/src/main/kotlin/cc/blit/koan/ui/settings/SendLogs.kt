package cc.blit.koan.ui.settings

import android.content.ClipData
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.Process
import androidx.core.content.FileProvider
import cc.blit.koan.BuildConfig
import cc.blit.koan.KoanApp
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject

/**
 * What a tester sends instead of an adb session: the app's own log since it
 * started, read with `logcat --pid` (which needs no permission for the app's
 * own process), under a few lines saying what build, phone and state it is.
 */
suspend fun shareLogs(context: Context, app: KoanApp) {
    val file = withContext(Dispatchers.IO) { writeReport(context, app) }
    val uri = FileProvider.getUriForFile(context, "${context.packageName}.files", file)
    val send = Intent(Intent.ACTION_SEND)
        .setType("text/plain")
        .putExtra(Intent.EXTRA_SUBJECT, "kōan log, ${BuildConfig.VERSION_NAME} (${BuildConfig.COMMIT})")
        .putExtra(Intent.EXTRA_STREAM, uri)
        .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    send.clipData = ClipData.newRawUri(null, uri)
    context.startActivity(Intent.createChooser(send, "Send logs").addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
}

private fun writeReport(context: Context, app: KoanApp): File {
    val mirror = app.mirror
    val devices = mirror.devices.value
    val controlled = devices.target?.let { id -> devices.devices.firstOrNull { it.id == id }?.name ?: id }
    val saved = runCatching {
        JSONObject(File(context.filesDir, "koan/devices.json").readText()).optJSONObject("target")?.toString()
    }.getOrNull()
    val connection = mirror.connection.value
    val header = buildString {
        appendLine("kōan ${BuildConfig.VERSION_NAME} (${BuildConfig.COMMIT})")
        appendLine("Phone: ${Build.MANUFACTURER} ${Build.MODEL}, Android ${Build.VERSION.RELEASE} (SDK ${Build.VERSION.SDK_INT})")
        appendLine("Controlling: ${controlled ?: "this phone"}; saved target: ${saved ?: "none"}")
        appendLine("Output: ${mirror.outputs.value?.current ?: "unknown"}")
        appendLine("Now playing: ${mirror.nowPlaying.value?.let { "${it.state} ${it.entry?.title ?: ""}" } ?: "nothing"}")
        appendLine(
            "Server: ${connection?.serverKind ?: "none"} ${connection?.serverVersion.orEmpty()}, " +
                "linked ${connection?.linked}, offline ${connection?.offline}, " +
                "local network blocked ${connection?.localNetworkBlocked}",
        )
        appendLine()
    }
    val log = runCatching {
        val logcat = ProcessBuilder("logcat", "-d", "-v", "threadtime", "--pid=${Process.myPid()}", "-t", "5000")
            .redirectErrorStream(true)
            .start()
        logcat.inputStream.bufferedReader().use { it.readText() }
    }.getOrElse { "The log could not be read: $it" }
    val dir = File(context.cacheDir, "logs").apply { mkdirs() }
    return File(dir, "koan-log.txt").apply { writeText(header + log) }
}
