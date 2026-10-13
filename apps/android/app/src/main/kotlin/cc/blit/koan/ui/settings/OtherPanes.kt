package cc.blit.koan.ui.settings

import android.content.ClipData
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.ClipEntry
import androidx.compose.ui.platform.LocalClipboard
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.ffi.Assistants
import cc.blit.koan.ffi.KoanException
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanField
import cc.blit.koan.ui.theme.KoanFormRow
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanSegmented
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

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
                KoanFormRow("Pre-amp", inset) {
                    Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.m), verticalAlignment = Alignment.CenterVertically) {
                        val db = s.preAmpDb
                        KoanButton(
                            "Quieter",
                            { model.edit { it.copy(preAmpDb = (db - 0.5).coerceAtLeast(-15.0)) } },
                            kind = KoanButtonKind.Icon,
                            icon = KoanIcons.subtract,
                            enabled = db > -15.0,
                        )
                        KoanText("%.1f dB".format(db), role = KoanType.Control)
                        KoanButton(
                            "Louder",
                            { model.edit { it.copy(preAmpDb = (db + 0.5).coerceAtMost(15.0)) } },
                            kind = KoanButtonKind.Icon,
                            icon = KoanIcons.add,
                            enabled = db < 15.0,
                        )
                    }
                }
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
    val connect = { if (token.isNotBlank() && !busy) model.connectScrobbling(token) { token = "" } }

    // Read once per page: the address only changes with the server.
    val assistants by produceState<Result<Assistants>?>(null, connection.offers(ASSISTANTS)) {
        if (connection.offers(ASSISTANTS)) value = runCatching { app.mirror.engine().assistants() }
    }

    SettingsPage(Pane.Integrations.title, state) {
        assistants?.let { assistantsSection(it, uri::openUri) }
        if (!offered) return@SettingsPage
        header("Scrobbling")
        when (val scrobbling = state.scrobbling) {
            Scrobbling.Checking -> line("Checking…", KoanTone.Muted)
            Scrobbling.Unknown -> item { ButtonRow { KoanButton("Try again", model::loadScrobbling, kind = KoanButtonKind.Bordered) } }
            is Scrobbling.Known -> {
                val c = scrobbling.connection
                if (c != null) {
                    line("Scrobbling to ListenBrainz as ${c.account}")
                    val refused = c.error
                    if (refused != null) {
                        line(refused, KoanTone.Bad)
                        line("Disconnect, then connect again with a current token. Plays recorded meanwhile are kept and sent.", KoanTone.Muted)
                    } else if (c.pending > 0) {
                        line("${c.pending} ${if (c.pending == 1L) "play" else "plays"} waiting to be sent", KoanTone.Muted)
                    }
                    item {
                        ButtonRow {
                            KoanButton("Disconnect", model::disconnectScrobbling, kind = KoanButtonKind.Bordered, enabled = !busy, destructive = true)
                        }
                    }
                } else {
                    item {
                        KoanFormRow("User token", inset) {
                            KoanField(
                                token,
                                { token = it },
                                Modifier.fillMaxWidth(),
                                placeholder = "ListenBrainz user token",
                                keyboardOptions = KeyboardOptions(
                                    keyboardType = KeyboardType.Password,
                                    autoCorrectEnabled = false,
                                    imeAction = ImeAction.Done,
                                ),
                                keyboardActions = KeyboardActions(onDone = { connect() }),
                                visualTransformation = PasswordVisualTransformation(),
                            )
                        }
                    }
                    item {
                        ButtonRow {
                            KoanButton("Find your token", { uri.openUri("https://listenbrainz.org/settings/") }, kind = KoanButtonKind.Link)
                            KoanButton("Connect", connect, kind = KoanButtonKind.Bordered, enabled = !busy && token.isNotBlank())
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

/**
 * Where an AI assistant connects to the server: its MCP address, and the
 * server's own page on adding it. Signing the assistant in happens there.
 */
private fun LazyListScope.assistantsSection(found: Result<Assistants>, open: (String) -> Unit) {
    header("Assistants")
    val a = found.getOrNull()
    if (a != null) {
        item { ValueRow("Address", a.mcpUrl) { CopyButton("Address", a.mcpUrl) } }
        item { ButtonRow { KoanButton("How to connect an assistant", { open(a.connectUrl) }, kind = KoanButtonKind.Link) } }
    }
    footer(
        (found.exceptionOrNull() as? KoanException)?.describe()
            ?: ("Add the address to Claude or another assistant that speaks MCP as a custom connector. It signs in " +
                "through your server, and can then search your library, make playlists and play music on your devices, as you."),
    )
}

/** Copies `text`, and says so for a moment. */
@Composable
internal fun CopyButton(what: String, text: String) {
    val clipboard = LocalClipboard.current
    val scope = rememberCoroutineScope()
    var copied by remember { mutableStateOf(false) }
    KoanButton(if (copied) "Copied" else "Copy", {
        scope.launch {
            clipboard.setClipEntry(ClipEntry(ClipData.newPlainText(what, text)))
            copied = true
            delay(1_500)
            copied = false
        }
    }, kind = KoanButtonKind.Bordered)
}

/**
 * The kōan theme or the platform's look, and colour from the record playing.
 * Written through the mirror, which re-reads `[appearance]` so the whole app
 * redraws in it at once.
 */
@Composable
fun AppearancePane(model: SettingsModel) {
    val mirror = koanApp().mirror
    val state by model.state.collectAsStateWithLifecycle()
    val appearance by mirror.appearance.collectAsStateWithLifecycle()
    val a = appearance ?: return SettingsPage(Pane.Appearance.title, state) {}
    SettingsPage(Pane.Appearance.title, state) {
        header("Theme")
        item {
            KoanSegmented(
                listOf("kōan" to true, "System" to false),
                a.koan,
                { koan -> mirror.setAppearance { setTheme(koan) } },
                inset.padding(vertical = KoanSpace.s),
            )
        }
        if (a.koan) {
            item { SwitchRow("Show icons", a.icons) { on -> mirror.setAppearance { setThemeIcons(on) } } }
        }
        footer("kōan is the site's look; System, the platform's own. Show icons puts icons beside the labels in the tabs and the buttons.")
        header("Colour")
        item { SwitchRow("Colours from the record", a.recordColours) { on -> mirror.setAppearance { setRecordColours(on) } } }
        footer(
            "The record playing colours the accent on selection and progress. Off, the accent is kōan's mint, in either theme.",
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
        item { ValueRow("Licence", "MIT") }
        item {
            ButtonRow {
                KoanButton("Source code", { uri.openUri(SOURCE) }, kind = KoanButtonKind.Link)
                KoanButton("koan.rocks", { uri.openUri("https://koan.rocks") }, kind = KoanButtonKind.Link)
            }
        }
        header("Open-source licences")
        LICENCES.forEach { (name, licence) ->
            item(name) {
                Column(inset.padding(vertical = KoanSpace.xs)) {
                    KoanText(name)
                    KoanText(licence, role = KoanType.Fine, tone = KoanTone.Muted)
                }
            }
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
    "Geist Mono" to "SIL Open Font License 1.1",
)
