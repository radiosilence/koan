package cc.blit.koan.ui.settings

import androidx.activity.ComponentActivity
import androidx.activity.compose.LocalActivity
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.R
import cc.blit.koan.ffi.ConnectionInfo
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.navigation.Tab
import cc.blit.koan.ui.navigation.nonce
import cc.blit.koan.ui.theme.LocalKoanTokens
import kotlinx.serialization.Serializable

/** The pages Settings goes into, as the iPhone's are. */
@Serializable
enum class Pane(val title: String) {
    Server("Server"),
    Account("Account"),
    Playback("Playback"),
    Downloads("Downloads"),
    Devices("Devices"),
    Integrations("Integrations"),
    Appearance("Appearance"),
    About("About"),
}

private val Pane.icon
    get() = when (this) {
        Pane.Server -> R.drawable.ic_pane_server
        Pane.Account -> R.drawable.ic_pane_account
        Pane.Playback -> R.drawable.ic_pane_speaker
        Pane.Downloads -> R.drawable.ic_pane_download
        Pane.Devices -> R.drawable.ic_play_on
        Pane.Integrations -> R.drawable.ic_pane_extensions
        Pane.Appearance -> R.drawable.ic_pane_appearance
        Pane.About -> R.drawable.ic_pane_info
    }

@Serializable
data class SettingsPaneRoute(val pane: Pane, val nonce: String = nonce()) : NavKey

fun EntryProviderScope<NavKey>.settingsEntries() {
    entry<Tab.Settings> { SettingsRoot() }
    entry<SettingsPaneRoute> { key ->
        val model = settingsModel()
        when (key.pane) {
            Pane.Server -> ServerPane(model)
            Pane.Account -> AccountPane(model)
            Pane.Playback -> PlaybackPane(model)
            Pane.Downloads -> DownloadsPane(model)
            Pane.Devices -> DevicesPane(model)
            Pane.Integrations -> IntegrationsPane(model)
            Pane.Appearance -> AppearancePane(model)
            Pane.About -> AboutPane(model)
        }
    }
}

/** The account model the whole activity shares: the sign-in page, Settings and opened links. */
@Composable
fun accountModel(): AccountModel {
    val app = koanApp()
    val activity = LocalActivity.current as ComponentActivity
    return viewModel(viewModelStoreOwner = activity) { AccountModel(app.mirror) }
}

/** The page's own copy of the settings, read again whenever the app comes back to the front. */
@Composable
private fun settingsModel(): SettingsModel {
    val app = koanApp()
    val model = viewModel { SettingsModel(app.mirror) }
    LifecycleResumeEffect(model) {
        model.reload()
        onPauseOrDispose {}
    }
    return model
}

@Composable
private fun SettingsRoot() {
    val app = koanApp()
    val model = settingsModel()
    val state by model.state.collectAsStateWithLifecycle()
    val connection by app.mirror.connection.collectAsStateWithLifecycle()
    val nav = LocalNavigator.current
    val signedIn = state.settings?.remoteSignedIn == true
    val panes = Pane.entries.filter {
        when (it) {
            Pane.Account -> signedIn && connection.offers(PASSWORDS)
            Pane.Integrations -> signedIn && connection?.scrobbling == true
            else -> true
        }
    }
    val version = appVersion(LocalContext.current)
    SettingsPage("Settings", state) {
        panes.forEach { pane ->
            item(pane.name) {
                ListItem(
                    headlineContent = { Text(pane.title) },
                    leadingContent = { Icon(painterResource(pane.icon), null) },
                    modifier = Modifier.clickable { nav.push(SettingsPaneRoute(pane)) },
                )
            }
        }
        item {
            Text(
                version,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(LocalKoanTokens.current.gutter),
            )
        }
    }
}

/**
 * The questions an account can ask from anywhere: switching to an invite's
 * account, letting another device sign in as this one, and what came of it.
 * Shown over sign-in and the tabs alike.
 */
@Composable
fun AccountPrompts(model: AccountModel) {
    val state by model.state.collectAsStateWithLifecycle()
    state.pendingInvite?.let { invite ->
        AlertDialog(
            onDismissRequest = model::dismissInvite,
            title = { Text("Switch accounts?") },
            text = {
                Text(
                    "kōan is signed in to another account. This invite signs in as ${invite.username} on " +
                        "${host(invite.server)}; tracks already synced stay in the library.",
                )
            },
            confirmButton = { TextButton({ model.join(invite) }) { Text("Switch") } },
            dismissButton = { TextButton(model::dismissInvite) { Text("Cancel") } },
        )
    }
    state.pendingPairing?.let { request ->
        AlertDialog(
            onDismissRequest = { model.settle(request, approve = false) },
            title = { Text("Sign in ${request.device}?") },
            text = {
                Column {
                    Text(request.origin)
                    Text(
                        "It will be signed in as ${request.username} on ${request.host}. Allow only a device you are " +
                            "setting up yourself: anyone can give a device any name.",
                        modifier = Modifier.padding(top = 12.dp),
                    )
                }
            },
            confirmButton = { TextButton({ model.settle(request, approve = true) }) { Text("Allow") } },
            dismissButton = { TextButton({ model.settle(request, approve = false) }) { Text("Decline") } },
        )
    }
    state.notice?.let { notice ->
        AlertDialog(
            onDismissRequest = model::dismissNotice,
            text = { Text(notice) },
            confirmButton = { TextButton(model::dismissNotice) { Text("OK") } },
        )
    }
}

/** The extension a koan server lists when an account can change its own password. */
internal const val PASSWORDS = "koanPasswords"

/** Whether the server listed `extension`. Features beyond Subsonic are used only where it does. */
internal fun ConnectionInfo?.offers(extension: String) = this?.extensions?.any { it.name == extension } == true

internal fun appVersion(context: android.content.Context): String {
    val info = context.packageManager.getPackageInfo(context.packageName, 0)
    return "kōan ${info.versionName}"
}
