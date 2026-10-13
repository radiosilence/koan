package cc.blit.koan.ui.settings

import android.text.format.DateUtils
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyListScope
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
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.ffi.ConnectedInfo
import cc.blit.koan.ffi.ConnectionInfo
import cc.blit.koan.ffi.Settings
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.theme.LocalKoanTokens

/**
 * Being found and controlled by koan apps on the same network, reaching
 * devices on networks that do not announce them, sharing this device with
 * other accounts, approving a device's sign-in, and the queue kept on the
 * server.
 */
@Composable
fun DevicesPane(model: SettingsModel) {
    val app = koanApp()
    val account = accountModel()
    val state by model.state.collectAsStateWithLifecycle()
    val connection by app.mirror.connection.collectAsStateWithLifecycle()
    var code by rememberSaveable { mutableStateOf("") }
    var address by rememberSaveable { mutableStateOf("") }
    var grantee by rememberSaveable { mutableStateOf("") }
    val s = state.settings ?: return SettingsPage(Pane.Devices.title, state) {}
    val c = connection

    SettingsPage(Pane.Devices.title, state) {
        if (s.remoteSignedIn && c?.pairing == true) {
            header("Pair a device")
            item {
                val approve = {
                    if (code.isNotBlank()) account.offerPairing(code)
                    code = ""
                }
                InlineField(code, { code = it }, "XXXX-XXXX", "Approve", approve, capitals = true)
            }
            footer(
                "A television or another device without a keyboard shows a code while it waits. Enter it here to sign " +
                    "it in as you, with a key of its own that can be revoked on the server.",
            )
        }
        if (s.remoteSignedIn) {
            header("Play queue")
            item { SwitchRow("Keep the queue on the server", s.playQueue, onChange = model::setServerQueue) }
            footer(
                "Saves this device's queue to your account on the server, where other apps can pick it up, and picks " +
                    "up a queue another app saved there when kōan starts. Moving music between kōan devices does not need it.",
            )
        }

        header("This device")
        item { SwitchRow("Discoverable on this network", s.devicesDiscoverable) { on -> model.edit { it.copy(devicesDiscoverable = on) } } }
        item {
            ChoiceRow(
                "Devices on this network",
                s.devicesNearbyControl,
                listOf("Full control" to "full", "Playback only" to "playback"),
            ) { v -> model.edit { it.copy(devicesNearbyControl = v) } }
        }
        c?.listeningPort?.let { port -> item { ValueRow("Listening on port", port.toString()) } }
        footer(
            "Other kōan apps on this network can see and control what plays here. Full control adds the output, " +
                "preset, volume and moving the music; Playback only is play and the queue. Neither reaches your library. " +
                "On a network shared with strangers, choose Playback only.",
        )

        connected(model, s, c)

        header("Devices by address")
        s.devicesAddresses.forEach { addr ->
            item(addr) {
                ListItem(
                    headlineContent = { Text(addr, fontFamily = FontFamily.Monospace) },
                    trailingContent = {
                        TextButton({ model.edit { it.copy(devicesAddresses = it.devicesAddresses - addr) } }) { Text("Remove") }
                    },
                )
            }
        }
        item {
            val add = {
                val a = address.trim()
                if (a.isNotEmpty() && a !in s.devicesAddresses) model.edit { it.copy(devicesAddresses = it.devicesAddresses + a) }
                address = ""
            }
            InlineField(address, { address = it }, "host or host:port", "Add", add, uri = true)
        }
        footer("For networks that do not announce devices, such as a tailnet. The port is 5626 unless the other device says otherwise.")

        if (c?.sharing == true) {
            header("Shared with other accounts")
            c.sharedWith.forEach { name ->
                item("shared-$name") {
                    ListItem(
                        headlineContent = { Text(name) },
                        trailingContent = { TextButton({ model.shareDevice(name, allow = false) }) { Text("Stop sharing") } },
                    )
                }
            }
            item {
                val share = {
                    if (grantee.isNotBlank()) model.shareDevice(grantee, allow = true)
                    grantee = ""
                }
                InlineField(grantee, { grantee = it }, "Their username on this server", "Share", share)
            }
            // The server's accounts matching what is typed, not shared with yet.
            val typed = grantee.trim().lowercase()
            if (typed.isNotEmpty()) {
                c.shareAccounts
                    .filter { it !in c.sharedWith && it != grantee && it.lowercase().startsWith(typed) }
                    .take(5)
                    .forEach { name ->
                        item("suggest-$name") {
                            ListItem(headlineContent = { Text(name) }, modifier = Modifier.clickable { grantee = name })
                        }
                    }
            }
            c.shareError?.let { error ->
                item { Text(error, color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(horizontal = LocalKoanTokens.current.gutter)) }
            }
            footer(
                "From any network, as their own account, they can see and control what this device plays, including the " +
                    "output, preset and volume. Nothing of your library, playlists, history or settings.",
            )
        }
    }

    state.replacingQueue?.let { saved ->
        AlertDialog(
            onDismissRequest = { model.confirmServerQueue(replace = false) },
            title = { Text("Replace this queue?") },
            text = {
                val tracks = if (saved.savedTracks == 1u) "track" else "tracks"
                Text(
                    "Your server has a queue of ${saved.savedTracks} $tracks saved by ${saved.savedBy}. Keeping the queue on " +
                        "the server replaces the one on this device with it.",
                )
            },
            confirmButton = { TextButton({ model.confirmServerQueue(replace = true) }) { Text("Replace with the server's queue") } },
            dismissButton = { TextButton({ model.confirmServerQueue(replace = false) }) { Text("Cancel") } },
        )
    }
}

/**
 * Who is connected to this device and whom it is connected to, each with a
 * way to end it where this device can.
 */
private fun LazyListScope.connected(model: SettingsModel, s: Settings, c: ConnectionInfo?) {
    val connections = c?.connections.orEmpty()
    val held = c?.held.orEmpty()
    if (connections.isEmpty() && held.isEmpty() && s.devicesRefused.isEmpty()) return
    header("Connected now")
    // By place: two of the account's devices can share a name and have
    // nothing else to tell them apart.
    connections.forEachIndexed { i, connection ->
        item("connection-$i") {
            ListItem(
                headlineContent = { Text(connection.name) },
                supportingContent = { Text(detail(connection)) },
                trailingContent = connection.key?.let {
                    {
                        Row {
                            if (!connection.own && connection.owner == null && connection.addr != null) {
                                TextButton({ model.endConnection(connection, refuse = true) }) { Text("Refuse") }
                            }
                            TextButton({ model.endConnection(connection, refuse = false) }) { Text("Disconnect") }
                        }
                    }
                },
            )
        }
    }
    held.forEach { h ->
        item("held-${h.addr}") {
            ListItem(
                headlineContent = { Text(h.name ?: h.addr) },
                supportingContent = { Text("Disconnected until allowed back") },
                trailingContent = { TextButton({ model.allowHeld(h) }) { Text("Allow") } },
            )
        }
    }
    s.devicesRefused.forEach { addr ->
        item("refused-$addr") {
            ListItem(
                headlineContent = { Text(addr, fontFamily = FontFamily.Monospace) },
                supportingContent = { Text("Refused") },
                trailingContent = {
                    TextButton({ model.edit { it.copy(devicesRefused = it.devicesRefused - addr) } }) { Text("Allow") }
                },
            )
        }
    }
    footer(
        "A device that can control this one uses its battery while it does. Disconnect hangs up a device on this " +
            "network and keeps it from connecting again, and this device from connecting to it, until you allow it back " +
            "here, play on it, pick it in Play on, or kōan restarts. Refuse keeps a device that has not proved it is " +
            "yours out for good, by its address.",
    )
}

/** What a connection is, and since when. */
private fun detail(c: ConnectedInfo): String {
    val who = when {
        c.own -> "your device"
        c.owner != null -> "shared by ${c.owner}"
        else -> "not proved"
    }
    val what = when {
        c.viaServer && !c.inbound -> "Your server"
        c.viaServer -> "Your device, through the server"
        c.inbound -> "Can control this device on this network, $who"
        else -> "This device is connected to it on this network, $who"
    }
    val since = c.since ?: return what
    return "$what · since " + DateUtils.formatDateTime(
        null,
        since * 1000,
        if (DateUtils.isToday(since * 1000)) DateUtils.FORMAT_SHOW_TIME else DateUtils.FORMAT_SHOW_DATE or DateUtils.FORMAT_SHOW_TIME,
    )
}

/** A field with its one action beside it, run on the keyboard's action key too. */
@Composable
internal fun InlineField(
    value: String,
    onChange: (String) -> Unit,
    placeholder: String,
    action: String,
    onAction: () -> Unit,
    uri: Boolean = false,
    capitals: Boolean = false,
) {
    ButtonRow {
        OutlinedTextField(
            value,
            onChange,
            placeholder = { Text(placeholder) },
            singleLine = true,
            keyboardOptions = KeyboardOptions(
                keyboardType = if (uri) KeyboardType.Uri else KeyboardType.Text,
                capitalization = if (capitals) KeyboardCapitalization.Characters else KeyboardCapitalization.None,
                autoCorrectEnabled = false,
                imeAction = ImeAction.Done,
            ),
            keyboardActions = KeyboardActions(onDone = { onAction() }),
            modifier = Modifier.weight(1f).fillMaxWidth(),
        )
        Button(onAction, enabled = value.isNotBlank()) { Text(action) }
    }
}
