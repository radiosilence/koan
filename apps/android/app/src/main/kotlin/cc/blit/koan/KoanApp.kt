package cc.blit.koan

import android.app.Application
import android.os.StrictMode

class KoanApp : Application() {
    override fun onCreate() {
        super.onCreate()
        if (BuildConfig.STRICT_MODE) strictMode()
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
