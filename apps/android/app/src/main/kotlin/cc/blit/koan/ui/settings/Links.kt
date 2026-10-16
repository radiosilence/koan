package cc.blit.koan.ui.settings

import android.content.Intent
import androidx.activity.ComponentActivity
import androidx.activity.compose.LocalActivity
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.core.util.Consumer
import kotlinx.coroutines.launch

/**
 * What the app was opened with or handed while open. Invite and pairing
 * links (`koan.rocks/join/`, `koan.rocks/pair/`, `koan://join`) go to the
 * account, as does an invite shared as text. EQ files shared or opened with
 * the app, and other shared text, go to the import Settings' file picker
 * runs. The manifest's intent filters decide what arrives.
 */
@Composable
fun OpenedLinks(account: AccountModel, dsp: DspModel) {
    val activity = LocalActivity.current as ComponentActivity
    val scope = rememberCoroutineScope()
    val take: (Intent) -> Unit = { intent ->
        val files = sharedFiles(intent)
        val text = intent.getStringExtra(Intent.EXTRA_TEXT).takeIf { intent.action == Intent.ACTION_SEND }
        when {
            files.isNotEmpty() -> scope.launch { dsp.importPaths(copyIn(activity, files)) }
            text != null -> scope.launch { account.parseInvite(text)?.let(account::offer) ?: dsp.importText(text) }
            intent.action == Intent.ACTION_VIEW -> intent.data?.let(account::open)
        }
    }
    // The intent the activity was started with, once: not again when it is
    // recreated holding the same intent.
    var launchHandled by rememberSaveable { mutableStateOf(false) }
    LaunchedEffect(activity) {
        if (!launchHandled) {
            launchHandled = true
            activity.intent?.let(take)
        }
    }
    DisposableEffect(activity) {
        val listener = Consumer<Intent> { take(it) }
        activity.addOnNewIntentListener(listener)
        onDispose { activity.removeOnNewIntentListener(listener) }
    }
}
