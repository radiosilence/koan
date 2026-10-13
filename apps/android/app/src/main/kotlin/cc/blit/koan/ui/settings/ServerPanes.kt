package cc.blit.koan.ui.settings

import android.content.ClipData
import android.text.format.DateUtils
import android.text.format.Formatter
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.platform.ClipEntry
import androidx.compose.ui.platform.LocalClipboard
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.ffi.ConnectionInfo
import cc.blit.koan.ffi.SyncPhase
import cc.blit.koan.ffi.SyncProgress
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.theme.LocalKoanTokens
import kotlinx.coroutines.launch

/** The server signed in to, syncing it, signing out, and what it offers. */
@Composable
fun ServerPane(model: SettingsModel) {
    val app = koanApp()
    val account = accountModel()
    val state by model.state.collectAsStateWithLifecycle()
    val connection by app.mirror.connection.collectAsStateWithLifecycle()
    val tasks by app.mirror.tasks.collectAsStateWithLifecycle()
    val progress by app.mirror.sync.collectAsStateWithLifecycle()
    val clipboard = LocalClipboard.current
    val scope = rememberCoroutineScope()
    var signingOut by remember { mutableStateOf(false) }
    val s = state.settings ?: return SettingsPage(Pane.Server.title, state) {}

    SettingsPage(Pane.Server.title, state) {
        if (!s.remoteSignedIn) {
            item { Text("Not signed in.", Modifier.padding(LocalKoanTokens.current.gutter)) }
            return@SettingsPage
        }
        header("Signed in")
        item {
            // The address is what another device or app asks for, so a tap copies it.
            ValueRow("Server", s.remoteUrl) {
                scope.launch { clipboard.setClipEntry(ClipEntry(ClipData.newPlainText("Server", s.remoteUrl))) }
            }
        }
        item { ValueRow("User", s.remoteUsername) }
        item { ValueRow("Tracks", "%,d".format(s.remoteTracks.toLong())) }
        item { ValueRow("Last synced", s.remoteLastSync?.let(::ago) ?: "Never") }
        if (connection?.signInRefused == true) {
            item {
                Text(
                    "Your server refused kōan's sign-in. Sign out and sign in again.",
                    color = MaterialTheme.colorScheme.error,
                    modifier = Modifier.padding(horizontal = LocalKoanTokens.current.gutter),
                )
            }
        }
        progress?.let { p -> item { ValueRow("Syncing", describe(p)) } }
        item {
            ButtonRow {
                // Only syncs wait on each other. Signing out is a config write,
                // and greying it out while a sync runs strands you on a server
                // you are trying to leave.
                OutlinedButton(
                    model::syncNow,
                    enabled = SettingsModel.REMOTE !in state.running && !tasks.syncing,
                ) { Text("Sync") }
                OutlinedButton({ signingOut = true }) { Text("Sign out") }
            }
        }

        header("Automatic sync")
        item { SwitchRow("Keep the library in sync", s.autoSync) { on -> model.edit { it.copy(autoSync = on) } } }
        if (s.autoSync) {
            item {
                ChoiceRow(
                    "Every",
                    s.autoSyncIntervalMins,
                    listOf(
                        "Startup only" to 0uL,
                        "15 minutes" to 15uL,
                        "Hour" to 60uL,
                        "6 hours" to 360uL,
                        "Day" to 1440uL,
                    ),
                ) { mins -> model.edit { it.copy(autoSyncIntervalMins = mins) } }
            }
        }
        footer("Each sync asks the server only for what changed since the last.")

        serverOffers(connection)
    }

    if (signingOut) {
        AlertDialog(
            onDismissRequest = { signingOut = false },
            title = { Text("Sign out of ${host(s.remoteUrl)}?") },
            text = {
                Text(
                    "Keeping its tracks leaves records in the library that cannot be played until you sign in again.",
                )
            },
            confirmButton = {
                Column {
                    TextButton({
                        signingOut = false
                        model.signOut(forgetTracks = true, then = account::signedOut)
                    }) { Text("Sign out and forget its tracks") }
                    TextButton({
                        signingOut = false
                        model.signOut(forgetTracks = false, then = account::signedOut)
                    }) { Text("Sign out and keep its tracks") }
                    TextButton({ signingOut = false }) { Text("Cancel") }
                }
            },
        )
    }
}

/** What the server said it is when koan signed in, and what that turns on. */
private fun androidx.compose.foundation.lazy.LazyListScope.serverOffers(c: ConnectionInfo?) {
    header("What the server offers")
    if (c == null || (c.serverKind == null && !c.openSubsonic)) {
        item { ListItem(headlineContent = { Text("Not reached yet") }) }
    } else {
        val name = when (val kind = c.serverKind) {
            "koan" -> "kōan"
            null -> "Subsonic"
            else -> kind.replaceFirstChar { it.uppercase() }
        }
        item { ValueRow("Server", listOfNotNull(name, c.serverVersion).joinToString(" ")) }
        item { ValueRow("OpenSubsonic", if (c.openSubsonic) "Yes" else "No") }
        item {
            ValueRow(
                "Your devices",
                when {
                    !c.devices -> "Not offered"
                    c.linked -> "Connected"
                    else -> "Offered, not connected"
                },
            )
        }
        c.extensions.forEach { e ->
            item { ValueRow(e.name, e.versions.joinToString(", ") { "v$it" }) }
        }
    }
    footer(
        "Asked when kōan signs in and whenever its link to the server reconnects. Features beyond Subsonic are used " +
            "only where the server lists them.",
    )
}

/** Downloads kept on this phone, how many run at once, and offline mode. */
@Composable
fun DownloadsPane(model: SettingsModel) {
    val app = koanApp()
    val context = LocalContext.current
    val state by model.state.collectAsStateWithLifecycle()
    val connection by app.mirror.connection.collectAsStateWithLifecycle()
    val cacheBytes by app.mirror.cacheBytes.collectAsStateWithLifecycle()
    // The limit as typed, committed whole: "5" on the way to "50GB" is not a
    // limit anyone set.
    var draft by rememberSaveable { mutableStateOf<String?>(null) }
    val s = state.settings ?: return SettingsPage(Pane.Downloads.title, state) {}
    val commit = {
        draft?.let { typed -> if (typed.trim() != s.cacheLimit) model.edit { it.copy(cacheLimit = typed.trim()) } }
        draft = null
    }

    SettingsPage(Pane.Downloads.title, state) {
        if (s.remoteSignedIn) {
            item {
                SwitchRow("Offline mode", connection?.offlineManual == true, onChange = model::setOffline)
            }
            footer(
                "Shows only what is on this phone. It turns on by itself when your server cannot be reached, and off " +
                    "again when it can.",
            )
        }
        header("Downloads")
        item {
            ListItem(
                headlineContent = { Text("Parallel downloads") },
                trailingContent = {
                    Row(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.CenterVertically) {
                        val n = s.downloadWorkers.toInt()
                        OutlinedButton({ model.edit { it.copy(downloadWorkers = (n - 1).coerceAtLeast(1).toUInt()) } }, enabled = n > 1) {
                            Text("−")
                        }
                        Text("$n")
                        OutlinedButton({ model.edit { it.copy(downloadWorkers = (n + 1).coerceAtMost(16).toUInt()) } }, enabled = n < 16) {
                            Text("+")
                        }
                    }
                },
            )
        }
        item {
            OutlinedTextField(
                draft ?: s.cacheLimit,
                { draft = it },
                label = { Text("Cache limit") },
                placeholder = { Text("e.g. 50GB, blank for no limit") },
                singleLine = true,
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
                keyboardActions = KeyboardActions(onDone = { commit() }),
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = LocalKoanTokens.current.gutter, vertical = 4.dp)
                    .onFocusChanged { if (!it.isFocused) commit() },
            )
        }
        item {
            ListItem(
                headlineContent = { Text("Using") },
                supportingContent = { Text(Formatter.formatShortFileSize(context, cacheBytes)) },
                trailingContent = {
                    OutlinedButton(
                        { model.clearCache(context) },
                        enabled = SettingsModel.DOWNLOADS !in state.running,
                    ) { Text("Clear") }
                },
            )
        }
    }
}

/** The signed-in account's password, where the server lets it be changed here. */
@Composable
fun AccountPane(model: SettingsModel) {
    val state by model.state.collectAsStateWithLifecycle()
    var changing by remember { mutableStateOf(false) }
    var current by remember { mutableStateOf("") }
    var new by remember { mutableStateOf("") }
    val s = state.settings ?: return SettingsPage(Pane.Account.title, state) {}
    SettingsPage(Pane.Account.title, state) {
        header("Password")
        item { ValueRow("User", s.remoteUsername) }
        item { ButtonRow { OutlinedButton({ changing = true }) { Text("Change password") } } }
        footer(PASSWORD_NOTE)
    }
    if (changing) {
        val close = {
            changing = false
            current = ""
            new = ""
        }
        AlertDialog(
            onDismissRequest = close,
            title = { Text("Change your password") },
            text = {
                Column {
                    Text(PASSWORD_NOTE, style = MaterialTheme.typography.bodySmall)
                    OutlinedTextField(
                        current,
                        { current = it },
                        label = { Text("Current password") },
                        singleLine = true,
                        visualTransformation = PasswordVisualTransformation(),
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                    )
                    OutlinedTextField(
                        new,
                        { new = it },
                        label = { Text("New password") },
                        singleLine = true,
                        visualTransformation = PasswordVisualTransformation(),
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                    )
                }
            },
            confirmButton = {
                Button({
                    model.changePassword(current, new) {}
                    close()
                }, enabled = current.isNotEmpty() && new.isNotEmpty()) { Text("Change") }
            },
            dismissButton = { TextButton(close) { Text("Cancel") } },
        )
    }
}

private const val PASSWORD_NOTE =
    "This device stays signed in. Your other devices, and other apps using this account, will have to sign in again."

private fun describe(p: SyncProgress): String {
    val what = when (p.phase) {
        SyncPhase.ALBUMS -> "records"
        SyncPhase.TRACKS -> "tracks"
        SyncPhase.ARTISTS -> "artists"
        SyncPhase.FINISHING -> return "Finishing"
    }
    val done = "%,d".format(p.done.toLong())
    return p.total?.let { "$done of ${"%,d".format(it.toLong())} $what" } ?: "$done $what"
}

/** "5 minutes ago", from Unix seconds. */
private fun ago(seconds: Long): String {
    val now = System.currentTimeMillis()
    if (now - seconds * 1000 < DateUtils.MINUTE_IN_MILLIS) return "Just now"
    return DateUtils.getRelativeTimeSpanString(seconds * 1000, now, DateUtils.MINUTE_IN_MILLIS).toString()
}
