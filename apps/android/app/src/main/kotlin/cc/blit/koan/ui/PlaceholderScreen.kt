package cc.blit.koan.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.engine.KoanProcess
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.service.BitPerfectStatus
import cc.blit.koan.state.ServerStatus
import cc.blit.koan.state.SettingsModel
import kotlinx.coroutines.delay
import uniffi.koan_ffi.PlayState

/**
 * Phase 2's only screen: engine state, server sign-in and what is playing,
 * all read from the mirror. Nothing here calls the engine; a button calls a
 * model method, which returns at once and does the work on the engine's
 * dispatcher.
 */
@Composable
fun PlaceholderScreen() {
    val runtime by KoanProcess.runtime.collectAsStateWithLifecycle()
    val failure by KoanProcess.failure.collectAsStateWithLifecycle()
    Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
        Column(
            Modifier.safeDrawingPadding().padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text("kōan", style = MaterialTheme.typography.headlineMedium)
            val ready = runtime
            when {
                failure != null -> Text("The engine did not start: $failure")
                ready == null -> Text("Starting the engine…")
                else -> Started(ready)
            }
        }
    }
}

@Composable
private fun Started(runtime: KoanRuntime) {
    val settings by runtime.settings.settings.collectAsStateWithLifecycle()
    val connection by runtime.mirror.connection.collectAsStateWithLifecycle()
    val status = SettingsModel.status(settings, connection?.signInRefused == true, connection?.linked == true)
    when (status) {
        ServerStatus.Loading -> Text("Reading settings…")
        ServerStatus.SignedOut -> SignIn(runtime)
        is ServerStatus.Refused -> {
            Text("${status.url} refused kōan's sign-in.")
            SignIn(runtime)
        }
        is ServerStatus.SignedIn ->
            Text("Signed in to ${status.url} as ${status.username}" + if (status.linked) " (linked)" else "")
    }
    NowPlayingCard(runtime)
}

@Composable
private fun SignIn(runtime: KoanRuntime) {
    var url by rememberSaveable { mutableStateOf("") }
    var user by rememberSaveable { mutableStateOf("") }
    var password by rememberSaveable { mutableStateOf("") }
    val busy by runtime.settings.signingIn.collectAsStateWithLifecycle()
    val error by runtime.settings.error.collectAsStateWithLifecycle()
    OutlinedTextField(url, { url = it }, Modifier.fillMaxWidth(), label = { Text("Server URL") }, singleLine = true)
    OutlinedTextField(user, { user = it }, Modifier.fillMaxWidth(), label = { Text("Username") }, singleLine = true)
    OutlinedTextField(password, { password = it }, Modifier.fillMaxWidth(), label = { Text("Password") }, singleLine = true)
    Button(onClick = { runtime.settings.signIn(url, user, password) }, enabled = !busy) {
        Text(if (busy) "Signing in…" else "Sign in")
    }
    error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
}

@Composable
private fun NowPlayingCard(runtime: KoanRuntime) {
    val playback by runtime.mirror.playback.collectAsStateWithLifecycle()
    val playhead by runtime.mirror.playhead.collectAsStateWithLifecycle()
    val bitPerfect by KoanProcess.bitPerfect.collectAsStateWithLifecycle()
    val entry = playback.entry
    // The position is derived from the anchor, a second at a time while it
    // moves; nothing arrives from the engine to make it tick.
    var shown by remember { mutableLongStateOf(0L) }
    LaunchedEffect(playhead, playback.durationMs) {
        shown = playhead.at(withinMs = playback.durationMs.toLong())
        while (playhead.playing) {
            delay(1000)
            shown = playhead.at(withinMs = playback.durationMs.toLong())
        }
    }
    if (entry == null) {
        Text("Nothing queued.")
    } else {
        Text(entry.title, style = MaterialTheme.typography.titleLarge)
        Text("${entry.artist} — ${entry.album}")
        Text("${clock(shown)} / ${clock(playback.durationMs.toLong())}")
        playback.format?.let {
            Text("${it.codec} ${it.sampleRate} Hz" + (it.bitDepth?.let { d -> " / $d-bit" } ?: ""))
        }
        Text(describe(bitPerfect))
    }
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        val player = runtime.player
        Button(onClick = player::previous) { Text("Previous") }
        Button(onClick = player::togglePlayPause) { Text(if (playback.state == PlayState.PLAYING) "Pause" else "Play") }
        Button(onClick = player::next) { Text("Next") }
    }
}

private fun describe(status: BitPerfectStatus): String = when (status) {
    BitPerfectStatus.Exclusive -> "Bit-perfect (exclusive)"
    BitPerfectStatus.MixerBitPerfect -> "Bit-perfect (mixer)"
    is BitPerfectStatus.No -> "Not bit-perfect: ${status.reason}"
}

private fun clock(ms: Long): String {
    val total = ms / 1000
    return "%d:%02d".format(total / 60, total % 60)
}
