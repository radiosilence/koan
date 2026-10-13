package cc.blit.koan

import android.Manifest
import android.content.ComponentName
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.media3.session.MediaController
import androidx.media3.session.SessionToken
import cc.blit.koan.playback.PlaybackService
import cc.blit.koan.ui.AppShell
import cc.blit.koan.ui.KoanRoot
import com.google.common.util.concurrent.ListenableFuture

class MainActivity : ComponentActivity() {
    private var controller: ListenableFuture<MediaController>? = null

    private val localNetwork =
        registerForActivityResult(ActivityResultContracts.RequestPermission(), ::localNetworkAnswered)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent { KoanRoot { AppShell() } }
        // Android 17 keeps other devices and renderers out of sight until the
        // person lets the app onto the local network.
        if (savedInstanceState == null &&
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.CINNAMON_BUN &&
            checkSelfPermission(Manifest.permission.ACCESS_LOCAL_NETWORK) != PackageManager.PERMISSION_GRANTED
        ) {
            localNetwork.launch(Manifest.permission.ACCESS_LOCAL_NETWORK)
        }
    }

    private fun localNetworkAnswered(granted: Boolean) {
        (application as KoanApp).mirror.launch {
            nearbyBlocked(!granted)
            if (granted) linkNudge()
        }
    }

    /**
     * Connect to the playback service while the app is open. The connection
     * is what starts it; from there Media3 keeps it in the foreground while
     * the engine plays, whether or not the app is.
     */
    override fun onStart() {
        super.onStart()
        controller = MediaController.Builder(
            this,
            SessionToken(this, ComponentName(this, PlaybackService::class.java)),
        ).buildAsync()
    }

    override fun onStop() {
        (application as KoanApp).player.saveOnLeaving()
        controller?.let(MediaController::releaseFuture)
        controller = null
        super.onStop()
    }
}
