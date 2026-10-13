package cc.blit.koan.ui.settings

import android.content.Intent
import androidx.activity.ComponentActivity
import androidx.activity.compose.LocalActivity
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.core.util.Consumer

/**
 * Invite and pairing links (`koan.rocks/join/`, `koan.rocks/pair/`,
 * `koan://join`) the app was opened with, or handed while open, given to the
 * account. The manifest's intent filters decide which links arrive.
 */
@Composable
fun OpenedLinks(account: AccountModel) {
    val activity = LocalActivity.current as ComponentActivity
    // The link the activity was started with, once: not again when it is
    // recreated holding the same intent.
    var launchHandled by rememberSaveable { mutableStateOf(false) }
    LaunchedEffect(activity) {
        if (!launchHandled) {
            launchHandled = true
            activity.intent?.viewed()?.let(account::open)
        }
    }
    DisposableEffect(activity) {
        val listener = Consumer<Intent> { intent -> intent.viewed()?.let(account::open) }
        activity.addOnNewIntentListener(listener)
        onDispose { activity.removeOnNewIntentListener(listener) }
    }
}

private fun Intent.viewed() = data?.takeIf { action == Intent.ACTION_VIEW }
