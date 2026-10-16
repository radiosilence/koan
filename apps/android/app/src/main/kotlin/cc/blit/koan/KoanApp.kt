package cc.blit.koan

import android.app.ActivityManager
import android.app.Application
import android.os.StrictMode
import cc.blit.koan.art.ArtImageLoader
import coil3.ImageLoader
import coil3.PlatformContext
import coil3.SingletonImageLoader
import java.io.File

class KoanApp : Application(), SingletonImageLoader.Factory {
    // Resolved before StrictMode is on: asking for a directory is disk access,
    // and the image loader is built on Main by the first sleeve that is drawn.
    private lateinit var artDirectory: File
    private var memoryClass = 0

    override fun onCreate() {
        super.onCreate()
        artDirectory = ArtImageLoader.directory(cacheDir)
        memoryClass = getSystemService(ActivityManager::class.java).memoryClass
        if (BuildConfig.STRICT_MODE) strictMode()
    }

    /** Artwork: the fetcher for the engine's cover bytes, a memory cache sized from the heap budget, a disk cache. */
    override fun newImageLoader(context: PlatformContext): ImageLoader {
        return ArtImageLoader.build(this, memoryClass, artDirectory)
    }

    /**
     * Debug builds die on the first thing that blocks the main thread: disk,
     * network, a slow call. That is the point of it, and it is the issue's rule:
     * nothing on Main waits on the engine, a file or the network. VM
     * violations are logged, not fatal, because a leak in a library should not
     * stop a build from running.
     */
    private fun strictMode() {
        StrictMode.setThreadPolicy(
            StrictMode.ThreadPolicy.Builder().detectAll().penaltyLog().penaltyDeath().build(),
        )
        StrictMode.setVmPolicy(StrictMode.VmPolicy.Builder().detectAll().penaltyLog().build())
    }
}
