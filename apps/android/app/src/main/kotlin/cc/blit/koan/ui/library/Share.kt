package cc.blit.koan.ui.library

import android.content.Context
import android.content.Intent
import android.widget.Toast
import cc.blit.koan.KoanApp
import cc.blit.koan.ffi.KoanException
import cc.blit.koan.ffi.Share
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * A public link to `playable` from the koan server, handed to the share sheet.
 *
 * Only tracks the server knows can go in a link, since it points at the
 * server; a pick mixing local-only files shares what it can and says how much
 * it left out. A failure says why, in the server's words.
 */
fun KoanApp.share(context: Context, playable: Playable) = mirror.launch {
    val result = runCatching { createShare(playable.trackIds(this), playable.name) }
    withContext(Dispatchers.Main) {
        result
            .onSuccess { share ->
                context.startActivity(shareIntent(share, playable.name))
                skippedNote(share)?.let { Toast.makeText(context, it, Toast.LENGTH_LONG).show() }
            }
            .onFailure { e ->
                val why = (e as? KoanException.BadArgument)?.detail ?: "The server could not make a link."
                Toast.makeText(context, why, Toast.LENGTH_LONG).show()
            }
    }
}

fun shareIntent(share: Share, name: String): Intent =
    Intent.createChooser(
        Intent(Intent.ACTION_SEND)
            .setType("text/plain")
            .putExtra(Intent.EXTRA_SUBJECT, name)
            .putExtra(Intent.EXTRA_TEXT, share.url),
        null,
    ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)

/** What a partial link left out, or nothing when it holds everything. */
fun skippedNote(share: Share): String? = when (share.skipped) {
    0u -> null
    else -> "${count(share.skipped.toLong(), "track")} only on this device left out of the link."
}
