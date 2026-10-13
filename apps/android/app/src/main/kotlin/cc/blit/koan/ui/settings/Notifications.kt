package cc.blit.koan.ui.settings

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.platform.LocalContext
import androidx.core.content.ContextCompat
import androidx.core.content.edit
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.ui.koanApp

/**
 * Ask to post notifications the first time music plays, on Android 13 and
 * later, where the media notification needs it. Asked once: playing is when
 * the question makes sense, and a refusal is the person's to undo in the
 * system's settings.
 */
@Composable
fun AskForNotificationsOnFirstPlay() {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return
    val context = LocalContext.current
    val playhead by koanApp().mirror.playhead.collectAsStateWithLifecycle()
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {}
    LaunchedEffect(playhead.playing) {
        if (!playhead.playing || !shouldAsk(context)) return@LaunchedEffect
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit { putBoolean(ASKED, true) }
        launcher.launch(Manifest.permission.POST_NOTIFICATIONS)
    }
}

private fun shouldAsk(context: Context): Boolean {
    val granted = ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) ==
        PackageManager.PERMISSION_GRANTED
    return !granted && !context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean(ASKED, false)
}

private const val PREFS = "koan.ui"
private const val ASKED = "notificationsAsked"
