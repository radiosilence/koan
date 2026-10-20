package cc.blit.koan.ui.settings

import android.os.SystemClock
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.compose.LocalActivity
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.AlertDialog
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.ffi.ConnectionInfo
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.navigation.Tab
import cc.blit.koan.ui.navigation.nonce
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanNavRow
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import cc.blit.koan.ui.theme.Rainbow
import cc.blit.koan.ui.theme.SecretTaps
import kotlinx.serialization.Serializable

/** The pages Settings goes into, as the iPhone's are. */
@Serializable
enum class Pane(val title: String) {
    Server("Server"),
    Account("Account"),
    People("People"),
    Playback("Playback"),
    Eq("EQ"),
    Downloads("Downloads"),
    Devices("Devices"),
    Integrations("Integrations"),
    Appearance("Appearance"),
    About("About"),
}

private val Pane.icon
    get() = when (this) {
        Pane.Server -> KoanIcons.server
        Pane.Account -> KoanIcons.account
        Pane.People -> KoanIcons.people
        Pane.Playback -> KoanIcons.speaker
        Pane.Eq -> KoanIcons.tuning
        Pane.Downloads -> KoanIcons.downloads
        Pane.Devices -> KoanIcons.devices
        Pane.Integrations -> KoanIcons.extensions
        Pane.Appearance -> KoanIcons.appearance
        Pane.About -> KoanIcons.info
    }

@Serializable
data class SettingsPaneRoute(val pane: Pane, val nonce: String = nonce()) : NavKey

fun EntryProviderScope<NavKey>.settingsEntries() {
    entry<Tab.Settings> { SettingsRoot() }
    entry<DspProfileRoute> { key -> DspProfilePage(key.name, settingsModel()) }
    entry<AutoEqRoute> { key -> AutoEqPage(key.query, settingsModel()) }
    entry<AutoEqMakerRoute> { key -> AutoEqMakerPage(key.maker, settingsModel()) }
    entry<SettingsPaneRoute> { key ->
        val model = settingsModel()
        when (key.pane) {
            Pane.Server -> ServerPane(model)
            Pane.Account -> AccountPane(model)
            Pane.People -> PeoplePane(model)
            Pane.Playback -> PlaybackPane(model)
            Pane.Eq -> EqPane(model)
            Pane.Downloads -> DownloadsPane(model)
            Pane.Devices -> DevicesPane(model)
            Pane.Integrations -> IntegrationsPane(model)
            Pane.Appearance -> AppearancePane(model)
            Pane.About -> AboutPane(model)
        }
    }
}

/** The profiles a share or Settings imports into: one for the activity, so a share lands on any page. */
@Composable
fun dspModel(): DspModel {
    val app = koanApp()
    val activity = LocalActivity.current as ComponentActivity
    return viewModel(viewModelStoreOwner = activity) { DspModel(app.mirror::engine) }
}

/** The account model the whole activity shares: the sign-in page, Settings and opened links. */
@Composable
fun accountModel(): AccountModel {
    val app = koanApp()
    val activity = LocalActivity.current as ComponentActivity
    return viewModel(viewModelStoreOwner = activity) { AccountModel(app.mirror::engine, app.activity) }
}

/** The page's own copy of the settings, read again whenever the app comes back to the front. */
@Composable
private fun settingsModel(): SettingsModel {
    val app = koanApp()
    val model = viewModel { SettingsModel(app.mirror::engine, app.activity) }
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
    // Asked for each account signed in as: the list loading is what says
    // the account is an admin's, and so whether there is a People page.
    val people = viewModel(key = "people-${state.settings?.remoteUrl}-${state.settings?.remoteUsername}") {
        PeopleModel(app.mirror::engine)
    }
    val admin = people.state.collectAsStateWithLifecycle().value.accounts != null
    val panes = Pane.entries.filter {
        when (it) {
            Pane.Account -> signedIn && accountShown(connection)
            Pane.People -> signedIn && admin
            Pane.Integrations -> signedIn && integrationsShown(connection)
            else -> true
        }
    }
    val context = LocalContext.current
    val version = appVersion(context)
    val appearance by app.mirror.appearance.collectAsStateWithLifecycle()
    val taps = remember { SecretTaps() }
    SettingsPage("Settings", state) {
        panes.forEach { pane ->
            item(pane.name) { KoanNavRow(pane.title, pane.icon, selected = false, onClick = { nav.push(SettingsPaneRoute(pane)) }) }
        }
        item {
            KoanText(
                version,
                Modifier
                    .clickable(interactionSource = null, indication = null) {
                        if (!taps.tap(SystemClock.uptimeMillis())) return@clickable
                        val on = appearance?.rainbow != true
                        app.mirror.setAppearance { setRainbow(on) }
                        Toast.makeText(context, (if (on) Rainbow.on else Rainbow.off).random(), Toast.LENGTH_SHORT).show()
                    }
                    .padding(KoanSpace.page),
                role = KoanType.Fine,
                tone = KoanTone.Muted,
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
            title = { KoanText("Switch accounts?", role = KoanType.TitleSmall, appText = true) },
            text = {
                KoanText(
                    "kōan is signed in to another account. This invite signs in as ${invite.username} on " +
                        "${host(invite.server)}; tracks already synced stay in the library.",
                )
            },
            confirmButton = { KoanButton("Switch", { model.join(invite) }) },
            dismissButton = { KoanButton("Cancel", model::dismissInvite) },
        )
    }
    state.pendingPairing?.let { request ->
        AlertDialog(
            onDismissRequest = { model.settle(request, approve = false) },
            title = { KoanText("Sign in ${request.device}?", role = KoanType.TitleSmall) },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(KoanSpace.m)) {
                    KoanText(request.origin)
                    KoanText(
                        "It will be signed in as ${request.username} on ${request.host}. Allow only a device you are " +
                            "setting up yourself: anyone can give a device any name.",
                    )
                }
            },
            confirmButton = { KoanButton("Allow", { model.settle(request, approve = true) }) },
            dismissButton = { KoanButton("Decline", { model.settle(request, approve = false) }) },
        )
    }
    state.notice?.let { notice ->
        AlertDialog(
            onDismissRequest = model::dismissNotice,
            text = { KoanText(notice) },
            confirmButton = { KoanButton("OK", model::dismissNotice) },
        )
    }
}

/** The extension a koan server lists when an account can change its own password. */
internal const val PASSWORDS = "koanPasswords"

/** The extension a koan server lists when it serves assistants over MCP. */
internal const val ASSISTANTS = "koanMcp"

/**
 * Whether the Integrations page has anything on it: scrobbling, or
 * assistants, which a koan server lists only once it knows the public address
 * it is reached at (`server.url`), since MCP's sign-in goes through it.
 */
internal fun integrationsShown(c: ConnectionInfo?) = c?.scrobbling == true || c.offers(ASSISTANTS)

/** Whether the Account page has anything on it: a password to change, or keys to manage. */
internal fun accountShown(c: ConnectionInfo?) =
    c.offers(PASSWORDS) || c.offers(API_KEYS) || (c.offers(APP_PASSWORDS) && c?.offline != true)

/** Whether the server listed `extension`. Features beyond Subsonic are used only where it does. */
internal fun ConnectionInfo?.offers(extension: String) = this?.extensions?.any { it.name == extension } == true

internal fun appVersion(context: android.content.Context): String {
    val info = context.packageManager.getPackageInfo(context.packageName, 0)
    return "kōan ${info.versionName}"
}
