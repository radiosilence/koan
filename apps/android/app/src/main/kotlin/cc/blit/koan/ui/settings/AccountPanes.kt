package cc.blit.koan.ui.settings

import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.Intent
import android.graphics.Bitmap
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.FilterQuality
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.ClipEntry
import androidx.compose.ui.platform.LocalClipboard
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.core.net.toUri
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import cc.blit.koan.ffi.AccountRole
import cc.blit.koan.ffi.Invite
import cc.blit.koan.ffi.qrCode
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanField
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanPicker
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import kotlin.math.floor
import kotlinx.coroutines.launch

/** The extensions a koan server lists for an account's own keys to it. */
internal const val API_KEYS = "koanApiKeys"
internal const val APP_PASSWORDS = "koanAppPasswords"

/**
 * The signed-in account's own keys to the server: its password, the API keys
 * apps and devices sign in with, and app passwords for apps that cannot use a
 * key. Each where the server has it.
 */
@Composable
fun AccountPane(model: SettingsModel) {
    val app = koanApp()
    val state by model.state.collectAsStateWithLifecycle()
    val connection by app.mirror.connection.collectAsStateWithLifecycle()
    val keys = viewModel(key = "keys") { CredentialsModel.apiKeys(app.mirror::engine) }
    val passwords = viewModel(key = "app-passwords") { CredentialsModel.appPasswords(app.mirror::engine) }
    val keysState by keys.state.collectAsStateWithLifecycle()
    val passwordsState by passwords.state.collectAsStateWithLifecycle()
    var changing by remember { mutableStateOf(false) }
    var revoking by remember { mutableStateOf<Pair<CredentialsModel, Credential>?>(null) }
    var keyName by rememberSaveable { mutableStateOf("") }
    var passwordName by rememberSaveable { mutableStateOf("") }
    val s = state.settings ?: return SettingsPage(Pane.Account.title, state) {}

    SettingsPage(Pane.Account.title, state) {
        if (connection.offers(PASSWORDS)) {
            header("Password")
            item { ValueRow("User", s.remoteUsername) }
            item { ButtonRow { KoanButton("Change password", { changing = true }, kind = KoanButtonKind.Bordered) } }
            footer(PASSWORD_NOTE)
        }
        if (connection.offers(API_KEYS)) {
            credentials(
                "API keys",
                keysState,
                keyName,
                { keyName = it },
                "New key",
                create = { keys.create(keyName) { made -> if (made) keyName = "" } },
                revoke = { revoking = keys to it },
                note = "A key signs another Subsonic app in as you, without your password. Each device kōan is signed in on " +
                    "has one too; revoking it signs that device out.",
            )
        }
        // Only while the server can be reached, as on the iPhone.
        if (connection.offers(APP_PASSWORDS) && connection?.offline != true) {
            credentials(
                "App passwords",
                passwordsState,
                passwordName,
                { passwordName = it },
                "New app password",
                create = { passwords.create(passwordName) { made -> if (made) passwordName = "" } },
                revoke = { revoking = passwords to it },
                note = "For Subsonic apps that sign in only with a token and salt, and so cannot use an API key. Changing " +
                    "your password revokes them all.",
            )
        }
    }

    if (changing) ChangePassword(model) { changing = false }
    revoking?.let { (owner, credential) ->
        AlertDialog(
            onDismissRequest = { revoking = null },
            title = { KoanText("Revoke “${credential.name}”?", role = KoanType.TitleSmall) },
            text = { KoanText("Whatever signs in with it stops working.") },
            confirmButton = {
                KoanButton("Revoke", {
                    owner.revoke(credential)
                    revoking = null
                }, destructive = true)
            },
            dismissButton = { KoanButton("Cancel", { revoking = null }) },
        )
    }
    keysState.made?.let { NewSecret("Key", it, keys::dismissMade) }
    passwordsState.made?.let { NewSecret("App password", it, passwords::dismissMade) }
}

private fun LazyListScope.credentials(
    title: String,
    state: CredentialsState,
    name: String,
    onName: (String) -> Unit,
    action: String,
    create: () -> Unit,
    revoke: (Credential) -> Unit,
    note: String,
) {
    header(title)
    val items = state.items
    if (items == null && state.error == null) line("Loading…", KoanTone.Muted)
    items?.forEach { credential ->
        item("$title-${credential.id}") {
            Row(inset.fillMaxWidth().padding(vertical = KoanSpace.xs), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    KoanText(credential.name)
                    KoanText(used(credential), role = KoanType.Fine, tone = KoanTone.Muted)
                }
                if (credential.thisDevice) {
                    // Revoking it is signing out, which Sign out does properly.
                    KoanText("This device", tone = KoanTone.Muted, appText = true)
                } else {
                    KoanButton("Revoke", { revoke(credential) }, kind = KoanButtonKind.Compact, destructive = true)
                }
            }
        }
    }
    if (items != null) item { InlineField(name, onName, "The app it is for", action, onAction = create) }
    footer(state.error ?: note)
}

/** "Made 3 Oct 2026 · last used 12 Oct 2026", from Unix seconds. */
internal fun used(c: Credential): String {
    fun date(secs: Long) = java.text.DateFormat.getDateInstance(java.text.DateFormat.MEDIUM).format(java.util.Date(secs * 1000))
    val made = c.created?.let { "Made ${date(it)}" }
    val used = c.lastUsed?.let { "last used ${date(it)}" } ?: "never used"
    return listOfNotNull(made, used).joinToString(" · ")
}

@Composable
private fun ChangePassword(model: SettingsModel, close: () -> Unit) {
    var current by remember { mutableStateOf("") }
    var new by remember { mutableStateOf("") }
    val secret = KeyboardOptions(keyboardType = KeyboardType.Password, autoCorrectEnabled = false)
    AlertDialog(
        onDismissRequest = close,
        title = { KoanText("Change your password", role = KoanType.TitleSmall, appText = true) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(KoanSpace.s)) {
                KoanText(PASSWORD_NOTE, role = KoanType.Fine, tone = KoanTone.Muted)
                KoanField(current, { current = it }, Modifier.fillMaxWidth(), placeholder = "Current password", keyboardOptions = secret, visualTransformation = PasswordVisualTransformation())
                KoanField(new, { new = it }, Modifier.fillMaxWidth(), placeholder = "New password", keyboardOptions = secret, visualTransformation = PasswordVisualTransformation())
            }
        },
        confirmButton = {
            KoanButton("Change", {
                model.changePassword(current, new) {}
                close()
            }, kind = KoanButtonKind.Prominent, enabled = current.isNotEmpty() && new.isNotEmpty())
        },
        dismissButton = { KoanButton("Cancel", close) },
    )
}

/** A key or app password just made, shown this once. */
@Composable
private fun NewSecret(kind: String, made: MadeSecret, dismiss: () -> Unit) {
    val clipboard = LocalClipboard.current
    val scope = rememberCoroutineScope()
    var copied by remember { mutableStateOf(false) }
    AlertDialog(
        onDismissRequest = dismiss,
        title = { KoanText("$kind for ${made.name}", role = KoanType.TitleSmall) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(KoanSpace.m)) {
                SelectionContainer { KoanText(made.secret, tone = KoanTone.Strong) }
                KoanText(
                    "This is the only time it is shown. Copy it into the app now; if it is lost, revoke it and make another.",
                    role = KoanType.Fine,
                    tone = KoanTone.Muted,
                )
            }
        },
        confirmButton = { KoanButton("Done", dismiss) },
        dismissButton = {
            KoanButton(if (copied) "Copied" else "Copy", {
                scope.launch {
                    clipboard.setClipEntry(ClipEntry(ClipData.newPlainText(kind, made.secret)))
                    copied = true
                }
            }, kind = KoanButtonKind.Bordered)
        },
    )
}

internal const val PASSWORD_NOTE =
    "This device stays signed in. Your other devices, and other apps using this account, will have to sign in again."

/** The server's accounts, for its admins. */
@Composable
fun PeoplePane(model: SettingsModel) {
    val app = koanApp()
    val state by model.state.collectAsStateWithLifecycle()
    val connection by app.mirror.connection.collectAsStateWithLifecycle()
    val people = viewModel { PeopleModel(app.mirror::engine) }
    val p by people.state.collectAsStateWithLifecycle()
    var newName by rememberSaveable { mutableStateOf("") }
    var newRole by rememberSaveable { mutableStateOf(AccountRole.READONLY) }
    var deleting by remember { mutableStateOf<String?>(null) }
    var resetting by remember { mutableStateOf<String?>(null) }
    var settingPassword by remember { mutableStateOf<String?>(null) }
    val me = state.settings?.remoteUsername
    val roles = AccountRole.entries.map { it.label to it }

    SettingsPage(Pane.People.title, state) {
        header("People")
        val accounts = p.accounts
        if (accounts == null) line("Only a kōan server's admins manage its accounts.", KoanTone.Muted)
        accounts?.forEach { account ->
            item("person-${account.username}") {
                Row(inset.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(KoanSpace.s), verticalAlignment = Alignment.CenterVertically) {
                    KoanText(account.username, Modifier.weight(1f))
                    if (account.username == me) KoanText("you", tone = KoanTone.Muted, appText = true)
                    KoanPicker(roles, account.role, { people.setRole(account.username, it) })
                    PersonMenu(
                        account.username,
                        mine = account.username == me,
                        canSetPassword = connection.offers(PASSWORDS),
                        invite = { people.invite(account.username) },
                        setPassword = { settingPassword = account.username },
                        reset = { resetting = account.username },
                        delete = { deleting = account.username },
                    )
                }
            }
        }
        if (accounts != null) {
            item {
                ButtonRow {
                    KoanField(newName, { newName = it }, Modifier.weight(1f), placeholder = "Username", keyboardOptions = KeyboardOptions(autoCorrectEnabled = false))
                    KoanPicker(roles, newRole, { newRole = it })
                    KoanButton("Add", { people.create(newName, newRole) { ok -> if (ok) newName = "" } }, kind = KoanButtonKind.Bordered, enabled = newName.isNotBlank())
                }
            }
            footer(p.error ?: "Adding someone makes their invite: one link that sets kōan up with the account.")
        }
    }

    p.invite?.let { InviteDialog(it, people::dismissInvite) }
    deleting?.let { name ->
        Confirm("Delete $name?", "Their devices stop working, and their playlists and favourites go.", "Delete", destructive = true, onDismiss = { deleting = null }) {
            people.delete(name)
        }
    }
    resetting?.let { name ->
        Confirm("Give $name a new password?", "The invite carries the new password. Their devices will have to sign in again.", "New password", onDismiss = { resetting = null }) {
            people.invite(name, reset = true)
        }
    }
    settingPassword?.let { name ->
        var password by remember { mutableStateOf("") }
        AlertDialog(
            onDismissRequest = { settingPassword = null },
            title = { KoanText("Set a password for $name", role = KoanType.TitleSmall) },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(KoanSpace.s)) {
                    KoanText("Their devices will have to sign in again with it.", role = KoanType.Fine, tone = KoanTone.Muted)
                    KoanField(
                        password,
                        { password = it },
                        Modifier.fillMaxWidth(),
                        placeholder = "New password",
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, autoCorrectEnabled = false),
                        visualTransformation = PasswordVisualTransformation(),
                    )
                }
            },
            confirmButton = {
                KoanButton("Set", {
                    people.setPassword(name, password)
                    settingPassword = null
                }, enabled = password.isNotEmpty())
            },
            dismissButton = { KoanButton("Cancel", { settingPassword = null }) },
        )
    }
}

@Composable
private fun PersonMenu(
    name: String,
    mine: Boolean,
    canSetPassword: Boolean,
    invite: () -> Unit,
    setPassword: () -> Unit,
    reset: () -> Unit,
    delete: () -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    // Not for this account: a new password signs this app out too.
    val actions = listOf("Invite" to invite) + if (mine) {
        emptyList()
    } else {
        listOfNotNull(
            ("Set password…" to setPassword).takeIf { canSetPassword },
            "New password and invite…" to reset,
            "Delete" to delete,
        )
    }
    Box {
        KoanButton({ open = true }, kind = KoanButtonKind.Icon) { KoanIcon(KoanIcons.moreCircled, contentDescription = "More for $name") }
        DropdownMenu(open, { open = false }) {
            actions.forEach { (label, action) ->
                DropdownMenuItem(text = { KoanText(label, appText = true) }, onClick = {
                    open = false
                    action()
                })
            }
        }
    }
}

@Composable
private fun Confirm(title: String, message: String, action: String, destructive: Boolean = false, onDismiss: () -> Unit, onConfirm: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { KoanText(title, role = KoanType.TitleSmall) },
        text = { KoanText(message) },
        confirmButton = {
            KoanButton(action, {
                onConfirm()
                onDismiss()
            }, destructive = destructive)
        },
        dismissButton = { KoanButton("Cancel", onDismiss) },
    )
}

/** An invite, ready to send. The server sends no mail; this hands it to whatever the admin sends with. */
@Composable
private fun InviteDialog(invite: Invite, dismiss: () -> Unit) {
    val context = LocalContext.current
    val clipboard = LocalClipboard.current
    val scope = rememberCoroutineScope()
    // What was copied last, so only its button says so.
    var copied by remember { mutableStateOf<String?>(null) }
    val share = {
        val send = Intent(Intent.ACTION_SEND)
            .setType("text/plain")
            .putExtra(Intent.EXTRA_SUBJECT, invite.emailSubject)
            .putExtra(Intent.EXTRA_TEXT, invite.emailText)
        context.startActivity(Intent.createChooser(send, null))
    }
    AlertDialog(
        onDismissRequest = dismiss,
        title = { KoanText("Invite for ${invite.username}", role = KoanType.TitleSmall) },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(KoanSpace.m)) {
                KoanText(
                    "Opening the link on a phone, tablet or computer with kōan installed signs in and loads the library, on " +
                        "each device, for a week.",
                    tone = KoanTone.Muted,
                )
                InviteCode(invite.link)
                FlowRow(horizontalArrangement = Arrangement.spacedBy(KoanSpace.m), verticalArrangement = Arrangement.spacedBy(KoanSpace.s)) {
                    KoanButton("Send invite…", share, kind = KoanButtonKind.Prominent)
                    // Only mail apps answer a mailto:. The subject and body go as extras alone,
                    // since some apps would show both copies; with no mail app, the share sheet.
                    KoanButton("Open in Mail", {
                        val mail = Intent(Intent.ACTION_SENDTO, "mailto:".toUri())
                            .putExtra(Intent.EXTRA_SUBJECT, invite.emailSubject)
                            .putExtra(Intent.EXTRA_TEXT, invite.emailText)
                        try {
                            context.startActivity(mail)
                        } catch (e: ActivityNotFoundException) {
                            share()
                        }
                    })
                    // The formatted email, for pasting into a message composed elsewhere.
                    KoanButton(if (copied == "email") "Copied" else "Copy email", {
                        scope.launch {
                            clipboard.setClipEntry(ClipEntry(ClipData.newHtmlText("Invite", invite.emailText, invite.emailHtml)))
                            copied = "email"
                        }
                    }, kind = KoanButtonKind.Bordered)
                    KoanButton(if (copied == "link") "Copied" else "Copy link", {
                        scope.launch {
                            clipboard.setClipEntry(ClipEntry(ClipData.newPlainText("Invite", invite.link)))
                            copied = "link"
                        }
                    }, kind = KoanButtonKind.Bordered)
                }
                invite.password?.let { password ->
                    KoanText("For other Subsonic apps, shown this once:", role = KoanType.Fine, tone = KoanTone.Muted)
                    SelectionContainer {
                        Column {
                            KoanText("Server: ${invite.server}", role = KoanType.Meta)
                            KoanText("Username: ${invite.username}", role = KoanType.Meta)
                            KoanText("Password: $password", role = KoanType.Meta)
                        }
                    }
                }
            }
        },
        confirmButton = { KoanButton("Done", dismiss) },
    )
}

/**
 * The invite link as a QR code, for a phone beside this one to scan. Dark on white whatever the theme, since
 * scanners expect that contrast; each module a whole number of pixels, so the edges stay crisp.
 */
@Composable
private fun InviteCode(link: String) {
    val code = remember(link) {
        qrCode(link)?.let { code ->
            val width = code.width.toInt()
            val pixels = IntArray(width * width) {
                if (code.dark[it]) android.graphics.Color.BLACK else android.graphics.Color.WHITE
            }
            Bitmap.createBitmap(pixels, width, width, Bitmap.Config.ARGB_8888).asImageBitmap()
        }
    } ?: return
    Column(
        Modifier.fillMaxWidth(),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(KoanSpace.s),
    ) {
        Canvas(
            Modifier.size(192.dp)
                .background(Color.White)
                .padding(KoanSpace.m)
                .semantics { contentDescription = "QR code" },
        ) {
            val module = floor(size.minDimension / code.width).toInt()
            val side = module * code.width
            val inset = ((size.minDimension - side) / 2).toInt()
            drawImage(
                code,
                dstOffset = IntOffset(inset, inset),
                dstSize = IntSize(side, side),
                filterQuality = FilterQuality.None,
            )
        }
        KoanText("Scan with a phone's camera to open it there.", role = KoanType.Meta, tone = KoanTone.Muted)
    }
}
