package cc.blit.koan

import android.Manifest
import android.content.ComponentName
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.LaunchedEffect
import androidx.core.content.ContextCompat
import androidx.media3.session.MediaController
import androidx.media3.session.SessionToken
import cc.blit.koan.service.KoanPlaybackService
import cc.blit.koan.ui.PlaceholderScreen

/**
 * A single screen that shows the engine is up and what it is playing. The
 * real pages are phase 3; this exists so the service, the mirror and the
 * models have something to be seen through.
 *
 * It does not own anything: the engine is the playback service's. Connecting
 * a [MediaController] is how this activity starts and binds the service.
 */
class MainActivity : ComponentActivity() {
    private var controller: MediaController? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            val askNotifications = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {}
            LaunchedEffect(Unit) {
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    askNotifications.launch(Manifest.permission.POST_NOTIFICATIONS)
                }
            }
            PlaceholderScreen()
        }
    }

    override fun onStart() {
        super.onStart()
        val token = SessionToken(this, ComponentName(this, KoanPlaybackService::class.java))
        val future = MediaController.Builder(this, token).buildAsync()
        future.addListener({ controller = runCatching { future.get() }.getOrNull() }, ContextCompat.getMainExecutor(this))
    }

    override fun onStop() {
        controller?.release()
        controller = null
        super.onStop()
    }
}
