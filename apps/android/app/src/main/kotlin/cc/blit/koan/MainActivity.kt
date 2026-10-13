package cc.blit.koan

import android.content.ComponentName
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.media3.session.MediaController
import androidx.media3.session.SessionToken
import cc.blit.koan.playback.PlaybackService
import cc.blit.koan.ui.KoanScreen
import cc.blit.koan.ui.theme.KoanTheme
import com.google.common.util.concurrent.ListenableFuture

class MainActivity : ComponentActivity() {
    private var controller: ListenableFuture<MediaController>? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent { KoanTheme { KoanScreen() } }
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
        controller?.let(MediaController::releaseFuture)
        controller = null
        super.onStop()
    }
}
