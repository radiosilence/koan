package cc.blit.koan

import android.app.Application
import android.content.Context
import android.os.Build
import android.provider.Settings
import cc.blit.koan.art.ArtFetcher
import cc.blit.koan.art.ArtKeyer
import cc.blit.koan.art.followArtEvictions
import coil3.ImageLoader
import coil3.PlatformContext
import coil3.SingletonImageLoader
import coil3.disk.DiskCache
import okio.Path.Companion.toOkioPath
import cc.blit.koan.engine.EngineMirror
import cc.blit.koan.engine.PlayerModel
import cc.blit.koan.ffi.setConfigDir
import cc.blit.koan.playback.BitPerfectHost
import java.io.File

class KoanApp : Application(), SingletonImageLoader.Factory {
    lateinit var mirror: EngineMirror
        private set
    lateinit var player: PlayerModel
        private set

    override fun onCreate() {
        super.onCreate()
        System.loadLibrary("koan_ffi")
        attachContext(this)
        // Configuration, the library database and downloads. Excluded from
        // backup in the manifest: the downloads are lossless files, and the
        // rest is rebuilt by signing in.
        setConfigDir(File(filesDir, "koan").path)
        val name = Settings.Global.getString(contentResolver, Settings.Global.DEVICE_NAME) ?: Build.MODEL
        mirror = EngineMirror(name)
        player = PlayerModel(mirror)
        val output = BitPerfectHost(this)
        // The host first: a restored session that was playing resumes, and
        // opens its output through it.
        mirror.launch {
            setOutputHost(output)
            player.followSession()
        }
    }

    /** Coil, with covers fetched from the engine and their bytes kept in the app's cache directory. */
    override fun newImageLoader(context: PlatformContext): ImageLoader {
        val loader = ImageLoader.Builder(context)
            .components {
                add(ArtKeyer())
                add(ArtFetcher.Factory(mirror))
            }
            .diskCache {
                DiskCache.Builder()
                    .directory(cacheDir.resolve("art").toOkioPath())
                    .maxSizeBytes(256L * 1024 * 1024)
                    .build()
            }
            .build()
        followArtEvictions(mirror, loader)
        return loader
    }

    private companion object {
        /** The app's context, for the Android APIs the core reaches over JNI. */
        @JvmStatic
        external fun attachContext(context: Context)
    }
}
