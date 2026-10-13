package cc.blit.koan.ui.settings

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Button
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.theme.LocalKoanTokens

/** How music sounds: fades and ReplayGain. The route is the phone's to choose. */
@Composable
fun PlaybackPane(model: SettingsModel) {
    val state by model.state.collectAsStateWithLifecycle()
    val s = state.settings ?: return SettingsPage(Pane.Playback.title, state) {}
    SettingsPage(Pane.Playback.title, state) {
        header("Transport")
        item { SwitchRow("Fade on pause", s.fadeOnPause) { on -> model.edit { it.copy(fadeOnPause = on) } } }
        footer("Pause and resume ramp the volume over a moment instead of cutting.")

        header("Loudness")
        item {
            ChoiceRow(
                "ReplayGain",
                s.replaygain,
                listOf("Off" to "off", "Per track" to "track", "Per album" to "album"),
            ) { v -> model.edit { it.copy(replaygain = v) } }
        }
        if (s.replaygain != "off") {
            item {
                ListItem(
                    headlineContent = { Text("Pre-amp") },
                    trailingContent = {
                        Row(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.CenterVertically) {
                            val db = s.preAmpDb
                            OutlinedButton({ model.edit { it.copy(preAmpDb = (db - 0.5).coerceAtLeast(-15.0)) } }, enabled = db > -15.0) {
                                Text("−")
                            }
                            Text("%.1f dB".format(db))
                            OutlinedButton({ model.edit { it.copy(preAmpDb = (db + 0.5).coerceAtMost(15.0)) } }, enabled = db < 15.0) {
                                Text("+")
                            }
                        }
                    },
                )
            }
        }
        footer("Applies the gain written into the file's tags. Per album keeps the relative loudness within a record.")
    }
}

/**
 * What the server does on the account's behalf: scrobbling to ListenBrainz.
 * The token is handed over once and never comes back.
 */
@Composable
fun IntegrationsPane(model: SettingsModel) {
    val app = koanApp()
    val state by model.state.collectAsStateWithLifecycle()
    val connection by app.mirror.connection.collectAsStateWithLifecycle()
    val uri = LocalUriHandler.current
    var token by rememberSaveable { mutableStateOf("") }
    val offered = connection?.scrobbling == true
    LaunchedEffect(offered) { if (offered) model.loadScrobbling() }
    val busy = SettingsModel.SCROBBLING in state.running

    SettingsPage(Pane.Integrations.title, state) {
        header("Scrobbling")
        when (val scrobbling = state.scrobbling) {
            Scrobbling.Checking -> item { ListItem(headlineContent = { Text("Checking…") }) }
            Scrobbling.Unknown -> item { ButtonRow { OutlinedButton(model::loadScrobbling) { Text("Try again") } } }
            is Scrobbling.Known -> {
                val c = scrobbling.connection
                if (c != null) {
                    item { ListItem(headlineContent = { Text("Scrobbling to ListenBrainz as ${c.account}") }) }
                    val refused = c.error
                    if (refused != null) {
                        item {
                            ListItem(
                                headlineContent = { Text(refused, color = MaterialTheme.colorScheme.error) },
                                supportingContent = {
                                    Text("Disconnect, then connect again with a current token. Plays recorded meanwhile are kept and sent.")
                                },
                            )
                        }
                    } else if (c.pending > 0) {
                        item {
                            val plays = if (c.pending == 1L) "play" else "plays"
                            ListItem(headlineContent = { Text("${c.pending} $plays waiting to be sent") })
                        }
                    }
                    item { ButtonRow { OutlinedButton(model::disconnectScrobbling, enabled = !busy) { Text("Disconnect") } } }
                } else {
                    item {
                        val connect = { if (token.isNotBlank()) model.connectScrobbling(token) { token = "" } }
                        OutlinedTextField(
                            token,
                            { token = it },
                            label = { Text("ListenBrainz user token") },
                            singleLine = true,
                            visualTransformation = PasswordVisualTransformation(),
                            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, imeAction = ImeAction.Done),
                            keyboardActions = KeyboardActions(onDone = { connect() }),
                            modifier = Modifier.fillMaxWidth().padding(horizontal = LocalKoanTokens.current.gutter, vertical = 4.dp),
                        )
                    }
                    item {
                        ButtonRow {
                            TextButton({ uri.openUri("https://listenbrainz.org/settings/") }) { Text("Find your token") }
                            Button({ model.connectScrobbling(token) { token = "" } }, enabled = !busy && token.isNotBlank()) {
                                Text("Connect")
                            }
                        }
                    }
                }
            }
        }
        footer(
            "The server sends what you play to ListenBrainz, from every app signed in as you, your history included " +
                "when you connect. The token is kept on the server.",
        )
    }
}

/** The kōan theme or the platform's look, and colour from the record playing. */
@Composable
fun AppearancePane(model: SettingsModel) {
    val state by model.state.collectAsStateWithLifecycle()
    val a = state.appearance ?: return SettingsPage(Pane.Appearance.title, state) {}
    SettingsPage(Pane.Appearance.title, state) {
        header("Theme")
        item {
            SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth().padding(horizontal = LocalKoanTokens.current.gutter, vertical = 8.dp)) {
                listOf("kōan" to true, "System" to false).forEachIndexed { i, (label, koan) ->
                    SegmentedButton(
                        selected = a.koan == koan,
                        onClick = { model.setTheme(koan) },
                        shape = SegmentedButtonDefaults.itemShape(i, 2),
                    ) { Text(label) }
                }
            }
        }
        // Follows the choice, not the theme drawn now: a change waits for the
        // next launch, and the icons are its to set.
        if (a.koan) {
            item { SwitchRow("Show icons", a.icons, onChange = model::setIcons) }
        }
        footer(
            "kōan is the site's look; System, the platform's own. A change of theme takes effect the next time kōan " +
                "opens. Show icons puts icons beside the labels in the tabs and the buttons.",
        )
        header("Colour")
        item { SwitchRow("Colours from the record", a.recordColours, onChange = model::setRecordColours) }
        footer(
            "The record playing colours the page behind it, and the accent on selection and progress. Off, the accent " +
                "is kōan's mint, in either theme.",
        )
    }
}

/** The version, and whose work kōan is built on. */
@Composable
fun AboutPane(model: SettingsModel) {
    val state by model.state.collectAsStateWithLifecycle()
    val context = LocalContext.current
    val uri = LocalUriHandler.current
    SettingsPage(Pane.About.title, state) {
        item { ValueRow("Version", appVersion(context).removePrefix("kōan ")) }
        item {
            ListItem(
                headlineContent = { Text("Source code") },
                supportingContent = { Text("MIT licence") },
                modifier = Modifier.clickable { uri.openUri(SOURCE) },
            )
        }
        item {
            ListItem(
                headlineContent = { Text("Website") },
                supportingContent = { Text("koan.rocks") },
                modifier = Modifier.clickable { uri.openUri("https://koan.rocks") },
            )
        }
        header("Open-source licences")
        LICENCES.forEach { (name, licence) ->
            item(name) { ListItem(headlineContent = { Text(name) }, supportingContent = { Text(licence) }) }
        }
    }
}

private const val SOURCE = "https://github.com/radiosilence/koan"

/** What the app links directly, beyond the core. */
private val LICENCES = listOf(
    "AndroidX, Jetpack Compose, Media3" to "Apache License 2.0",
    "Kotlin, kotlinx.coroutines, kotlinx.serialization" to "Apache License 2.0",
    "Coil" to "Apache License 2.0",
    "JNA" to "Apache License 2.0 or LGPL 2.1",
    "UniFFI" to "Mozilla Public License 2.0",
)
