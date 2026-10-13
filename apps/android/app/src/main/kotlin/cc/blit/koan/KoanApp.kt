package cc.blit.koan

import android.app.Application
import android.content.Context
import android.os.Build
import android.provider.Settings
import cc.blit.koan.engine.Engine
import cc.blit.koan.ffi.setConfigDir
import java.io.File

class KoanApp : Application() {
    lateinit var engine: Engine
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
        engine = Engine(name)
    }

    private companion object {
        /** The app's context, for the Android APIs the core reaches over JNI. */
        @JvmStatic
        external fun attachContext(context: Context)
    }
}
