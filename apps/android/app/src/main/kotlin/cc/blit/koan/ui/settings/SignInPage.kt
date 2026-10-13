package cc.blit.koan.ui.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboard
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanField
import cc.blit.koan.ui.theme.KoanFormRow
import cc.blit.koan.ui.theme.KoanSegmented
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import cc.blit.koan.ui.theme.koanSurface
import kotlinx.coroutines.launch

/**
 * Sign in to a koan or Subsonic server: what the app shows until one is
 * signed in to. An address and password or API key, an invite pasted where
 * the address goes, or a code shown here for another device to approve.
 */
@Composable
fun SignInPage(model: AccountModel) {
    val state by model.state.collectAsStateWithLifecycle()
    var withCode by rememberSaveable { mutableStateOf(false) }
    Box(Modifier.fillMaxSize().koanSurface().safeDrawingPadding().imePadding()) {
        Column(
            Modifier
                .align(Alignment.TopCenter)
                .widthIn(max = 480.dp)
                .verticalScroll(rememberScrollState())
                .padding(KoanSpace.page),
            verticalArrangement = Arrangement.spacedBy(KoanSpace.m),
        ) {
            KoanText("kōan", Modifier.padding(top = KoanSpace.xxl), role = KoanType.Display, tone = KoanTone.Strong)
            val pairing = state.pairing
            when {
                pairing != null -> WaitingForApproval(pairing.code, pairing.link, onCancel = model::cancelPairing)
                withCode -> CodeForm(model, enabled = state.busy == null, onBack = { withCode = false })
                else -> PasswordForm(model, enabled = state.busy == null, onCode = { withCode = true })
            }
            state.error?.let { KoanText(it, tone = KoanTone.Bad) }
            state.busy?.let { Working(it) }
        }
    }
}

@Composable
private fun PasswordForm(model: AccountModel, enabled: Boolean, onCode: () -> Unit) {
    val clipboard = LocalClipboard.current
    val scope = rememberCoroutineScope()
    var url by rememberSaveable { mutableStateOf("") }
    var username by rememberSaveable { mutableStateOf("") }
    var secret by rememberSaveable { mutableStateOf("") }
    var withApiKey by rememberSaveable { mutableStateOf(false) }
    var notInvite by rememberSaveable { mutableStateOf(false) }
    val ready = enabled && url.isNotBlank() && username.isNotBlank() && secret.isNotEmpty()
    val submit = { if (ready) model.signIn(url, username, secret, withApiKey) }

    KoanText("Sign in to your Subsonic, Navidrome or kōan server.")
    KoanFormRow("Server") {
        KoanField(
            url,
            { typed ->
                url = typed
                // An invite, or an address with the account in it, pasted
                // where the address goes.
                scope.launch {
                    model.parseInvite(typed)?.let {
                        url = ""
                        model.offer(it)
                    }
                }
            },
            Modifier.fillMaxWidth(),
            placeholder = "https://music.example.com",
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, autoCorrectEnabled = false, imeAction = ImeAction.Next),
        )
    }
    KoanFormRow("Username") {
        KoanField(
            username,
            { username = it },
            Modifier.fillMaxWidth(),
            placeholder = "Username",
            keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, imeAction = ImeAction.Next),
        )
    }
    KoanSegmented(listOf("Password" to false, "API key" to true), withApiKey, { withApiKey = it })
    KoanFormRow(if (withApiKey) "API key" else "Password") {
        KoanField(
            secret,
            { secret = it },
            Modifier.fillMaxWidth(),
            placeholder = if (withApiKey) "API key" else "Password",
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, autoCorrectEnabled = false, imeAction = ImeAction.Go),
            keyboardActions = KeyboardActions(onGo = { submit() }),
            visualTransformation = PasswordVisualTransformation(),
        )
    }
    KoanButton("Sign in", submit, kind = KoanButtonKind.Prominent, enabled = ready)
    Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.l)) {
        KoanButton(
            "Paste invite",
            {
                scope.launch {
                    val text = clipboard.getClipEntry()?.clipData?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.text?.toString()
                    val invite = text?.let { model.parseInvite(it) }
                    notInvite = invite == null
                    invite?.let(model::offer)
                }
            },
            kind = KoanButtonKind.Link,
            enabled = enabled,
        )
        KoanButton("Sign in with a code", onCode, kind = KoanButtonKind.Link, enabled = enabled)
    }
    if (notInvite) KoanText("That isn't a kōan invite.", tone = KoanTone.Bad)
    KoanText(
        "An invite link opens kōan signed in. The account is checked against the server, then kept in kōan's own " +
            "storage on this phone.",
        role = KoanType.Fine,
        tone = KoanTone.Muted,
    )
}

/** The server to ask for a code, which another device signed in to it approves. */
@Composable
private fun CodeForm(model: AccountModel, enabled: Boolean, onBack: () -> Unit) {
    var server by rememberSaveable { mutableStateOf("") }
    val start = { if (enabled && server.isNotBlank()) model.startPairing(server) }
    KoanText(
        "kōan shows a code. Approve it from kōan on a device already signed in, under Settings → Devices, " +
            "and this phone signs in as that account.",
    )
    KoanFormRow("Server") {
        KoanField(
            server,
            { server = it },
            Modifier.fillMaxWidth(),
            placeholder = "music.example.com",
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, autoCorrectEnabled = false, imeAction = ImeAction.Go),
            keyboardActions = KeyboardActions(onGo = { start() }),
        )
    }
    KoanButton("Get a code", start, kind = KoanButtonKind.Prominent, enabled = enabled && server.isNotBlank())
    KoanButton("Sign in with a password instead", onBack, kind = KoanButtonKind.Link, enabled = enabled)
}

@Composable
private fun WaitingForApproval(code: String, link: String, onCancel: () -> Unit) {
    KoanText("Approve this code from kōan on a device already signed in to the server, under Settings → Devices.")
    SelectionContainer {
        KoanText(
            code,
            Modifier.fillMaxWidth().padding(vertical = KoanSpace.l).semantics { contentDescription = code.toList().joinToString(" ") },
            role = KoanType.Display,
            tone = KoanTone.Strong,
            textAlign = TextAlign.Center,
        )
    }
    KoanText("Or open this link there:", role = KoanType.Fine, tone = KoanTone.Muted)
    SelectionContainer { KoanText(link, role = KoanType.Fine, tone = KoanTone.Accent) }
    Working("Waiting for approval")
    KoanButton("Cancel", onCancel, kind = KoanButtonKind.Bordered)
}

/** Something under way, and what. */
@Composable
private fun Working(label: String) {
    Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.m), verticalAlignment = Alignment.CenterVertically) {
        CircularProgressIndicator(Modifier.size(16.dp), color = KoanTheme.color(KoanTone.Accent), strokeWidth = 1.5.dp)
        KoanText(label, tone = KoanTone.Muted)
    }
}
