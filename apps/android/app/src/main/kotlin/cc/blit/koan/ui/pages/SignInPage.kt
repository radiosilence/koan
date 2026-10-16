package cc.blit.koan.ui.pages

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.state.PairingState
import cc.blit.koan.state.ServerStatus
import cc.blit.koan.state.SettingsModel
import cc.blit.koan.theme.KoanButton
import cc.blit.koan.theme.KoanButtonKind
import cc.blit.koan.theme.KoanField
import cc.blit.koan.theme.KoanFootnote
import cc.blit.koan.theme.KoanRole
import cc.blit.koan.theme.KoanRule
import cc.blit.koan.theme.KoanSectionHeader
import cc.blit.koan.theme.KoanSegmented
import cc.blit.koan.theme.KoanSpace
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanTone

/**
 * Server setup and sign-in, shown by the shell in place of the tabs while the
 * server status is signed out or refused.
 *
 * Three ways in, as on the Apple apps: an account (password, or an API key made
 * in the web UI), an invite pasted where the address goes (the form fills in
 * and signs in, with nothing more asked), and a code this device shows for a
 * signed-in device to approve (`Pairing.swift`), for the places a keyboard is
 * a bother. The password and key live in this composition only: they are not
 * saved with the screen's state, and the engine takes them and forgets them.
 */
@Composable
fun SignInPage(runtime: KoanRuntime) {
    val model = runtime.settings
    val settings by model.settings.collectAsStateWithLifecycle()
    val connection by runtime.mirror.connection.collectAsStateWithLifecycle()
    val busy by model.signingIn.collectAsStateWithLifecycle()
    val error by model.error.collectAsStateWithLifecycle()
    val pairing by model.pairing.collectAsStateWithLifecycle()
    val pendingInvite by model.pendingInvite.collectAsStateWithLifecycle()
    val status = SettingsModel.status(settings, connection?.signInRefused == true, connection?.linked == true)

    var url by rememberSaveable { mutableStateOf("") }
    var user by rememberSaveable { mutableStateOf("") }
    var withKey by rememberSaveable { mutableStateOf(false) }
    // Deliberately not saveable: a secret does not belong in the saved instance state.
    var secret by remember { mutableStateOf("") }

    // A refused sign-in comes back to the account it had: the address and name are filled in.
    LaunchedEffect(status) {
        val current = settings
        if (status is ServerStatus.Refused && current != null && url.isEmpty()) {
            url = current.remoteUrl
            user = current.remoteUsername
        }
    }
    // A fresh page shows the configuration, so a refused account is known.
    LaunchedEffect(Unit) { model.refresh() }

    Column(
        Modifier.fillMaxSize().safeDrawingPadding().imePadding().verticalScroll(rememberScrollState()).padding(KoanSpace.page),
        verticalArrangement = Arrangement.spacedBy(KoanSpace.s),
    ) {
        KoanText("kōan", KoanRole.Display, KoanTone.Strong)
        KoanText("sign in to your music server", KoanRole.Body, KoanTone.Muted, maxLines = 2)
        if (status is ServerStatus.Refused) {
            KoanFootnote(REFUSED, tone = KoanTone.Bad)
        }

        KoanSectionHeader("subsonic or navidrome")
        KoanField(
            url,
            {
                url = it
                // An invite pasted where the address goes signs in by itself.
                if (it.length > MIN_INVITE && '/' in it) model.offerInvite(it, quiet = true, activity = runtime.activity)
            },
            placeholder = "server url",
            keyboard = KeyboardType.Uri,
            ime = ImeAction.Next,
        )
        KoanField(user, { user = it }, placeholder = "username", ime = ImeAction.Next)
        KoanSegmented(
            listOf("password" to false, "api key" to true), withKey, { withKey = it; secret = "" },
        )
        KoanField(
            secret, { secret = it },
            placeholder = if (withKey) "api key" else "password",
            secure = true,
            onDone = { if (!busy) model.signIn(url, user, secret, withKey, runtime.activity) },
        )
        KoanButton(
            if (busy) "signing in" else "sign in",
            { model.signIn(url, user, secret, withKey, runtime.activity) },
            kind = KoanButtonKind.Prominent,
            enabled = !busy && url.isNotBlank() && user.isNotBlank() && secret.isNotEmpty(),
        )
        KoanFootnote(
            "Paste an invite into the server url and kōan fills in the rest. The account is checked against " +
                "the server, then kept in a file only this app can read.",
        )

        pendingInvite?.let { invite ->
            KoanRule()
            val host = SettingsModel.hostOf(invite.server)
            KoanText("switch accounts?", KoanRole.Body, KoanTone.Strong)
            KoanFootnote(
                "kōan is signed in to another account. This invite signs in as ${invite.username} on $host; " +
                    "tracks already synced stay in the library.",
            )
            KoanButton("switch", { model.confirmInvite(runtime.activity) }, kind = KoanButtonKind.Bordered)
            KoanButton("cancel", model::dismissInvite, kind = KoanButtonKind.Link)
        }

        KoanSectionHeader("or use a code")
        when (val state = pairing) {
            PairingState.Idle, is PairingState.Failed -> {
                KoanFootnote(
                    "Sign this phone in from a device that is already signed in: it shows a code to enter there " +
                        "(Settings, Devices). Needs a koan server and the address above.",
                )
                (state as? PairingState.Failed)?.let { KoanFootnote(it.message, tone = KoanTone.Bad) }
                KoanButton(
                    "show a code", { model.startPairing(url) },
                    kind = KoanButtonKind.Bordered, enabled = url.isNotBlank() && !looksLikeInvite(url),
                )
            }
            PairingState.Starting -> KoanText("asking the server for a code", KoanRole.Body, KoanTone.Muted)
            is PairingState.Waiting -> {
                KoanText(state.code, KoanRole.Display, KoanTone.Accent, data = true)
                KoanFootnote("Enter this code on a signed-in device, or open ${state.link} there.")
                KoanButton("cancel", model::cancelPairing, kind = KoanButtonKind.Link)
            }
        }

        error?.let { KoanText(it, KoanRole.Meta, KoanTone.Bad, data = true, maxLines = 6) }
    }
}

private const val MIN_INVITE = 12
private const val REFUSED = "Your server refused kōan's sign-in. Sign in again."

/** A pasted invite is not a server address to ask for a code at. */
private fun looksLikeInvite(url: String): Boolean = url.contains("@") || url.contains("/join")
