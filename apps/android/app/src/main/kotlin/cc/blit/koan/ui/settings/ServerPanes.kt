package cc.blit.koan.ui.settings

import android.text.format.DateUtils
import android.text.format.Formatter
import java.util.Locale
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.ffi.ConnectionInfo
import cc.blit.koan.ffi.SyncPhase
import cc.blit.koan.ffi.SyncProgress
import cc.blit.koan.ffi.sizeInBytes
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanField
import cc.blit.koan.ui.theme.KoanFormRow
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType

/** The server signed in to, syncing it, signing out, and what it offers. */
@Composable
fun ServerPane(model: SettingsModel) {
    val app = koanApp()
    val account = accountModel()
    val state by model.state.collectAsStateWithLifecycle()
    val connection by app.mirror.connection.collectAsStateWithLifecycle()
    val tasks by app.mirror.tasks.collectAsStateWithLifecycle()
    val progress by app.mirror.sync.collectAsStateWithLifecycle()
    var signingOut by remember { mutableStateOf(false) }
    val s = state.settings ?: return SettingsPage(Pane.Server.title, state) {}

    SettingsPage(Pane.Server.title, state) {
        if (!s.remoteSignedIn) {
            line("Not signed in.", KoanTone.Muted)
            return@SettingsPage
        }
        header("Signed in")
        item {
            // The address is what another device or app asks for.
            ValueRow("Server", s.remoteUrl) { CopyButton("Server", s.remoteUrl) }
        }
        item { ValueRow("User", s.remoteUsername) }
        item { ValueRow("Tracks", "%,d".format(s.remoteTracks.toLong())) }
        item { ValueRow("Last synced", s.remoteLastSync?.let(::ago) ?: "Never") }
        progress?.let { p -> item { ValueRow("Syncing", describe(p)) } }
        if (connection?.signInRefused == true) {
            line("Your server refused kōan's sign-in. Sign out and sign in again.", KoanTone.Bad)
        }
        item {
            ButtonRow {
                // Only syncs wait on each other. Signing out is a config write,
                // and greying it out while a sync runs strands you on a server
                // you are trying to leave.
                KoanButton(
                    "Sync",
                    model::syncNow,
                    kind = KoanButtonKind.Bordered,
                    enabled = SettingsModel.REMOTE !in state.running && !tasks.syncing,
                )
                KoanButton("Sign out", { signingOut = true }, kind = KoanButtonKind.Bordered, destructive = true)
            }
        }

        // Offline is from a server, so it lives with the server.
        item { SwitchRow("Offline mode", connection?.offlineManual == true, onChange = model::setOffline) }
        footer(
            "Shows only what is on this phone. It turns on by itself when your server cannot be reached, and off " +
                "again when it can.",
        )

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
            title = { KoanText("Sign out of ${host(s.remoteUrl)}?", role = KoanType.TitleSmall) },
            text = {
                KoanText("Keeping its tracks leaves records in the library that cannot be played until you sign in again.")
            },
            confirmButton = {
                Column(horizontalAlignment = Alignment.End) {
                    KoanButton("Sign out and forget its tracks", {
                        signingOut = false
                        model.signOut(forgetTracks = true, then = account::signedOut)
                    }, destructive = true)
                    KoanButton("Sign out and keep its tracks", {
                        signingOut = false
                        model.signOut(forgetTracks = false, then = account::signedOut)
                    })
                    KoanButton("Cancel", { signingOut = false })
                }
            },
        )
    }
}

/** What the server said it is when koan signed in, and what that turns on. */
private fun LazyListScope.serverOffers(c: ConnectionInfo?) {
    header("What the server offers")
    if (c == null || (c.serverKind == null && !c.openSubsonic)) {
        line("Not reached yet", KoanTone.Muted)
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
            item("extension-${e.name}") {
                Row(inset.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                    KoanText(e.name, role = KoanType.Meta)
                    KoanText(e.versions.joinToString(", ") { "v$it" }, role = KoanType.Meta, tone = KoanTone.Muted)
                }
            }
        }
    }
    footer(
        "Asked when kōan signs in and whenever its link to the server reconnects. Features beyond Subsonic are used " +
            "only where the server lists them.",
    )
}

/** Downloads kept on this phone, and how many run at once. */
@Composable
fun DownloadsPane(model: SettingsModel) {
    val app = koanApp()
    val context = LocalContext.current
    val state by model.state.collectAsStateWithLifecycle()
    val cacheBytes by app.mirror.cacheBytes.collectAsStateWithLifecycle()
    // The limit as typed, committed whole: "5" on the way to "50" is not a
    // limit anyone set.
    var draft by rememberSaveable { mutableStateOf<String?>(null) }
    val s = state.settings ?: return SettingsPage(Pane.Downloads.title, state) {}
    // Read as the core reads it: none set is no limit, a limit of nothing no cache.
    val limitBytes = s.cacheLimit.takeIf { it.isNotBlank() }?.let { sizeInBytes(it) }
    val keeping = when {
        s.cacheLimit.isBlank() -> Keeping.Unlimited
        limitBytes == 0uL -> Keeping.Off
        else -> Keeping.Limited
    }
    val limitGb = limitBytes?.takeIf { it > 0uL }?.let { "%.2f".format(Locale.ROOT, it.toDouble() / GIB).trimEnd('0').trimEnd('.') }.orEmpty()
    val commit = {
        draft?.let { typed ->
            val gb = typed.trim().replace(',', '.').toDoubleOrNull()
            if (gb == null || gb <= 0) {
                model.report("A limit is a number of gigabytes, such as 50.")
            } else {
                val limit = "%.2f".format(Locale.ROOT, gb).trimEnd('0').trimEnd('.') + "GB"
                if (limit != s.cacheLimit) model.edit { it.copy(cacheLimit = limit) }
            }
        }
        draft = null
    }
    val note = listOfNotNull(
        "Nothing plays gaplessly, and each track waits for its download before it starts."
            .takeIf { s.downloadAhead == 0u },
        when (keeping) {
            Keeping.Off -> "Tracks are deleted once played. Downloads you ask for and favourite records stay."
            Keeping.Limited -> "Past the limit, the tracks played longest ago are deleted first. Favourite records stay."
            Keeping.Unlimited -> null
        },
    ).joinToString(" ")

    SettingsPage(Pane.Downloads.title, state) {
        header("Downloads")
        item {
            KoanFormRow("Parallel", inset) {
                Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.m), verticalAlignment = Alignment.CenterVertically) {
                    val n = s.downloadWorkers.toInt()
                    KoanButton(
                        "Fewer",
                        { model.edit { it.copy(downloadWorkers = (n - 1).coerceAtLeast(1).toUInt()) } },
                        kind = KoanButtonKind.Icon,
                        icon = KoanIcons.subtract,
                        enabled = n > 1,
                    )
                    KoanText("$n", role = KoanType.Control)
                    KoanButton(
                        "More",
                        { model.edit { it.copy(downloadWorkers = (n + 1).coerceAtMost(16).toUInt()) } },
                        kind = KoanButtonKind.Icon,
                        icon = KoanIcons.add,
                        enabled = n < 16,
                    )
                }
            }
        }
        item {
            ChoiceRow(
                "Download ahead",
                s.downloadAhead?.toInt() ?: -1,
                listOf("Playing track only" to 0, "1 track" to 1) + (2..10).map { "$it tracks" to it } + ("Whole queue" to -1),
            ) { n -> model.edit { it.copy(downloadAhead = n.takeIf { it >= 0 }?.toUInt()) } }
        }
        item {
            ChoiceRow(
                "Keep downloads",
                keeping,
                listOf("Up to a limit" to Keeping.Limited, "Without limit" to Keeping.Unlimited, "Only what plays next" to Keeping.Off),
            ) { k ->
                draft = null
                model.edit {
                    it.copy(
                        cacheLimit = when (k) {
                            Keeping.Limited -> "10GB"
                            Keeping.Unlimited -> ""
                            Keeping.Off -> "0"
                        },
                    )
                }
            }
        }
        if (keeping == Keeping.Limited) {
            item {
                KoanFormRow("Limit in GB", inset) {
                    KoanField(
                        draft ?: limitGb,
                        { draft = it },
                        Modifier.fillMaxWidth().onFocusChanged { if (!it.isFocused) commit() },
                        placeholder = "50",
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Decimal, imeAction = ImeAction.Done),
                        keyboardActions = KeyboardActions(onDone = { commit() }),
                    )
                }
            }
        }
        item {
            ValueRow("Using", Formatter.formatShortFileSize(context, cacheBytes)) {
                KoanButton(
                    "Clear",
                    { model.clearCache(context) },
                    kind = KoanButtonKind.Bordered,
                    enabled = SettingsModel.DOWNLOADS !in state.running,
                    destructive = true,
                )
            }
        }
        if (note.isNotEmpty()) footer(note)
    }
}

/** How much of what is downloaded to play the cache keeps. */
private enum class Keeping { Limited, Unlimited, Off }

private const val GIB = 1024.0 * 1024 * 1024

internal fun describe(p: SyncProgress): String {
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
internal fun ago(seconds: Long, now: Long = System.currentTimeMillis()): String {
    if (now - seconds * 1000 < DateUtils.MINUTE_IN_MILLIS) return "Just now"
    return DateUtils.getRelativeTimeSpanString(seconds * 1000, now, DateUtils.MINUTE_IN_MILLIS).toString()
}
