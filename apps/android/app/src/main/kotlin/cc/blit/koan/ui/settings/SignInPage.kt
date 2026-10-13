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
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
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
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.ui.theme.LocalKoanTokens
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
    Box(Modifier.fillMaxSize().safeDrawingPadding().imePadding()) {
        Column(
            Modifier
                .align(Alignment.TopCenter)
                .widthIn(max = 480.dp)
                .verticalScroll(rememberScrollState())
                .padding(LocalKoanTokens.current.gutter),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text("kōan", style = MaterialTheme.typography.displaySmall, modifier = Modifier.padding(top = 32.dp))
            val pairing = state.pairing
            when {
                pairing != null -> WaitingForApproval(pairing.code, pairing.link, onCancel = model::cancelPairing)
                withCode -> CodeForm(model, enabled = state.busy == null, onBack = { withCode = false })
                else -> PasswordForm(model, enabled = state.busy == null, onCode = { withCode = true })
            }
            state.error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            state.busy?.let { label ->
                Row(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.CenterVertically) {
                    CircularProgressIndicator(Modifier.padding(4.dp))
                    Text(label)
                }
            }
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

    Text("Sign in to your Subsonic, Navidrome or kōan server.", style = MaterialTheme.typography.bodyLarge)
    OutlinedTextField(
        url,
        { typed ->
            url = typed
            // An invite, or an address with the account in it, pasted where
            // the address goes.
            scope.launch {
                model.parseInvite(typed)?.let {
                    url = ""
                    model.offer(it)
                }
            }
        },
        label = { Text("Server URL") },
        placeholder = { Text("https://music.example.com") },
        singleLine = true,
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, autoCorrectEnabled = false, imeAction = ImeAction.Next),
        modifier = Modifier.fillMaxWidth(),
    )
    OutlinedTextField(
        username,
        { username = it },
        label = { Text("Username") },
        singleLine = true,
        keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, imeAction = ImeAction.Next),
        modifier = Modifier.fillMaxWidth(),
    )
    SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
        listOf("Password" to false, "API key" to true).forEachIndexed { i, (label, key) ->
            SegmentedButton(
                selected = withApiKey == key,
                onClick = { withApiKey = key },
                shape = SegmentedButtonDefaults.itemShape(i, 2),
            ) { Text(label) }
        }
    }
    OutlinedTextField(
        secret,
        { secret = it },
        label = { Text(if (withApiKey) "API key" else "Password") },
        singleLine = true,
        visualTransformation = PasswordVisualTransformation(),
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, imeAction = ImeAction.Go),
        keyboardActions = KeyboardActions(onGo = { submit() }),
        modifier = Modifier.fillMaxWidth(),
    )
    Button(submit, enabled = ready, modifier = Modifier.fillMaxWidth()) { Text("Sign in") }
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        TextButton(
            {
                scope.launch {
                    val text = clipboard.getClipEntry()?.clipData?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.text?.toString()
                    val invite = text?.let { model.parseInvite(it) }
                    notInvite = invite == null
                    invite?.let(model::offer)
                }
            },
            enabled = enabled,
        ) { Text("Paste invite") }
        TextButton(onCode, enabled = enabled) { Text("Sign in with a code") }
    }
    if (notInvite) Text("That isn't a kōan invite.", color = MaterialTheme.colorScheme.error)
    Text(
        "An invite link opens kōan signed in. The account is checked against the server, then kept in kōan's own " +
            "storage on this phone.",
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

/** The server to ask for a code, which another device signed in to it approves. */
@Composable
private fun CodeForm(model: AccountModel, enabled: Boolean, onBack: () -> Unit) {
    var server by rememberSaveable { mutableStateOf("") }
    val start = { if (enabled && server.isNotBlank()) model.startPairing(server) }
    Text(
        "kōan shows a code. Approve it from kōan on a device already signed in, under Settings → Devices, " +
            "and this phone signs in as that account.",
        style = MaterialTheme.typography.bodyLarge,
    )
    OutlinedTextField(
        server,
        { server = it },
        label = { Text("Server URL") },
        placeholder = { Text("music.example.com") },
        singleLine = true,
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, autoCorrectEnabled = false, imeAction = ImeAction.Go),
        keyboardActions = KeyboardActions(onGo = { start() }),
        modifier = Modifier.fillMaxWidth(),
    )
    Button(start, enabled = enabled && server.isNotBlank(), modifier = Modifier.fillMaxWidth()) { Text("Get a code") }
    TextButton(onBack, enabled = enabled) { Text("Sign in with a password instead") }
}

@Composable
private fun WaitingForApproval(code: String, link: String, onCancel: () -> Unit) {
    Text("Approve this code from kōan on a device already signed in to the server, under Settings → Devices.")
    SelectionContainer {
        Text(
            code,
            style = MaterialTheme.typography.displayMedium,
            fontFamily = FontFamily.Monospace,
            textAlign = TextAlign.Center,
            modifier = Modifier.fillMaxWidth().padding(vertical = 16.dp).semantics { contentDescription = code.toList().joinToString(" ") },
        )
    }
    Text("Or open this link there:", style = MaterialTheme.typography.bodySmall)
    SelectionContainer { Text(link, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.primary) }
    Row(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.CenterVertically) {
        CircularProgressIndicator(Modifier.padding(4.dp))
        Text("Waiting for approval")
    }
    OutlinedButton(onCancel) { Text("Cancel") }
}
