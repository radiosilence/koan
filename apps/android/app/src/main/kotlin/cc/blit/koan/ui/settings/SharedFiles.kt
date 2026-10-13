package cc.blit.koan.ui.settings

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.OpenableColumns
import androidx.core.content.IntentCompat
import java.io.File
import java.util.UUID
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * The files an intent hands over: one shared, several shared, or one opened
 * with the app. Links (`https`, `koan`) are not files, and are left to the
 * account.
 */
internal fun sharedFiles(intent: Intent): List<Uri> = when (intent.action) {
    Intent.ACTION_SEND -> listOfNotNull(IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java))
    Intent.ACTION_SEND_MULTIPLE ->
        IntentCompat.getParcelableArrayListExtra(intent, Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
    Intent.ACTION_VIEW -> listOfNotNull(intent.data?.takeIf { it.scheme == "content" || it.scheme == "file" })
    else -> emptyList()
}

/**
 * Copy content the system handed over into the app's cache, each under its
 * own name, where the core can read it by path: an import that stops to ask
 * for a sample rate still has its files when the answer comes. The name
 * matters, since the core tells a WAV from a zip from text by it.
 */
internal suspend fun copyIn(context: Context, uris: List<Uri>): List<String> = withContext(Dispatchers.IO) {
    val dir = File(context.cacheDir, "dsp-import/${UUID.randomUUID()}").apply { mkdirs() }
    uris.mapIndexedNotNull { i, uri ->
        val name = displayName(context, uri)?.let(::safeName) ?: "file-$i"
        val file = File(dir, name)
        runCatching {
            context.contentResolver.openInputStream(uri)?.use { input -> file.outputStream().use { input.copyTo(it) } }
                ?: return@mapIndexedNotNull null
            file.path
        }.getOrNull()
    }
}

private fun displayName(context: Context, uri: Uri): String? =
    if (uri.scheme == "file") {
        uri.lastPathSegment
    } else {
        context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
            if (cursor.moveToFirst()) cursor.getString(0) else null
        } ?: uri.lastPathSegment
    }

/** A file name that stays one file in one directory. */
internal fun safeName(name: String): String? =
    name.substringAfterLast('/').replace(Regex("[\\\\:*?\"<>|]"), "_").trim().takeIf { it.isNotEmpty() && it != "." && it != ".." }
