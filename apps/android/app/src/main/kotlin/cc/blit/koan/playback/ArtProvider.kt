package cc.blit.koan.playback

import android.content.ContentProvider
import android.content.ContentValues
import android.content.Context
import android.database.Cursor
import android.net.Uri
import android.os.ParcelFileDescriptor
import cc.blit.koan.art.Art
import cc.blit.koan.art.ArtSize
import coil3.SingletonImageLoader
import coil3.request.ImageRequest
import java.io.FileNotFoundException
import kotlinx.coroutines.runBlocking

/**
 * Covers for other processes: the system's media controls, the lock screen,
 * Android Auto and Wear read a session's artwork by URI, and can only open a
 * `content://` one. `content://<package>.art/album/<id>` (or `/track/<id>`)
 * is the record's cover at thumbnail size, read only.
 *
 * Served from Coil's disk cache, the same bytes the app draws; a cover not
 * cached yet is fetched from the engine into it first.
 */
class ArtProvider : ContentProvider() {
    override fun onCreate() = true

    override fun getType(uri: Uri) = artOf(uri)?.let { "image/*" }

    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor {
        if (mode != "r") throw SecurityException("covers are read only")
        val art = artOf(uri) ?: throw FileNotFoundException("not a cover: $uri")
        val context = context ?: throw FileNotFoundException(uri.toString())
        val loader = SingletonImageLoader.get(context)
        val disk = loader.diskCache ?: throw FileNotFoundException("no cover cache")
        fun cached() = disk.openSnapshot(art.key)?.use { snapshot ->
            // The descriptor stays valid if the cache later evicts the file.
            ParcelFileDescriptor.open(snapshot.data.toFile(), ParcelFileDescriptor.MODE_READ_ONLY)
        }
        cached()?.let { return it }
        // A binder thread, so waiting here blocks no one's main thread.
        runBlocking { loader.execute(ImageRequest.Builder(context).data(art).build()) }
        return cached() ?: throw FileNotFoundException("no cover for $uri")
    }

    override fun query(uri: Uri, projection: Array<out String>?, selection: String?, selectionArgs: Array<out String>?, sortOrder: String?): Cursor? = null

    override fun insert(uri: Uri, values: ContentValues?): Uri? = null

    override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?) = 0

    override fun update(uri: Uri, values: ContentValues?, selection: String?, selectionArgs: Array<out String>?) = 0

    companion object {
        fun authority(context: Context) = "${context.packageName}.art"

        /** The record's cover where the library knows the record, else the track's own. */
        fun uri(authority: String, albumId: Long?, trackId: Long?): Uri? = when {
            albumId != null -> Uri.Builder().scheme("content").authority(authority).appendPath("album").appendPath("$albumId").build()
            trackId != null -> Uri.Builder().scheme("content").authority(authority).appendPath("track").appendPath("$trackId").build()
            else -> null
        }

        fun artOf(uri: Uri): Art? {
            if (uri.scheme != "content" || uri.authority?.endsWith(".art") != true) return null
            val (kind, id) = uri.pathSegments.takeIf { it.size == 2 } ?: return null
            val number = id.toLongOrNull() ?: return null
            return when (kind) {
                "album" -> Art.album(number, ArtSize.Thumb)
                "track" -> Art.track(number, ArtSize.Thumb)
                else -> null
            }
        }
    }
}
