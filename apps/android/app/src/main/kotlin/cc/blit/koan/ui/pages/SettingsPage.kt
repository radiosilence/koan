package cc.blit.koan.ui.pages

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.ScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.engine.KoanProcess
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.service.BitPerfectStatus
import cc.blit.koan.state.ActivityTask
import cc.blit.koan.state.PairingRequest
import cc.blit.koan.state.Resource
import cc.blit.koan.state.SettingsModel
import cc.blit.koan.state.SettingsRules
import cc.blit.koan.theme.KoanButton
import cc.blit.koan.theme.KoanButtonKind
import cc.blit.koan.theme.KoanField
import cc.blit.koan.theme.KoanFootnote
import cc.blit.koan.theme.KoanFormRow
import cc.blit.koan.theme.KoanPicker
import cc.blit.koan.theme.KoanRole
import cc.blit.koan.theme.KoanRule
import cc.blit.koan.theme.KoanSectionHeader
import cc.blit.koan.theme.KoanSegmented
import cc.blit.koan.theme.KoanSpace
import cc.blit.koan.theme.KoanStepper
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanToggle
import cc.blit.koan.theme.KoanTone
import uniffi.koan_ffi.ConnectedInfo
import uniffi.koan_ffi.OutputChoice
import uniffi.koan_ffi.Settings

/** The panes, in the order the Mac's Settings window lists them. */
private enum class Pane(val label: String) {
    Library("library"),
    Server("server"),
    Playback("playback"),
    Devices("devices"),
    Appearance("appearance"),
}

/**
 * Settings (the Settings tab's root): five panes (library, server, playback,
 * devices, appearance) over `runtime.settings`, which commits a field when it
 * is edited and reads the configuration again each time the page resumes. A
 * line along the foot says what the last action did.
 *
 * Pages read flows and call model methods; nothing here touches the engine.
 */
@Composable
fun SettingsPage(runtime: KoanRuntime) {
    val model = runtime.settings
    val settings by model.settings.collectAsStateWithLifecycle()
    val error by model.error.collectAsStateWithLifecycle()
    val result by model.result.collectAsStateWithLifecycle()
    var pane by rememberSaveable { mutableStateOf(Pane.Server) }

    // Re-read on focus: a change made elsewhere shows up rather than being overwritten.
    LifecycleResumeEffect(model) {
        model.refresh()
        onPauseOrDispose { }
    }

    Column(Modifier.fillMaxSize()) {
        Column(Modifier.padding(horizontal = KoanSpace.page).padding(top = KoanSpace.page)) {
            KoanText("settings", KoanRole.Title, KoanTone.Strong)
            KoanSegmented(Pane.entries.map { it.label to it }, pane, { pane = it }, scrollable = true)
        }
        Column(
            Modifier.weight(1f).fillMaxWidth().imePadding().verticalScroll(rememberSaveable(pane, saver = ScrollState.Saver) { ScrollState(0) })
                .padding(horizontal = KoanSpace.page).padding(bottom = KoanSpace.page),
            verticalArrangement = Arrangement.spacedBy(KoanSpace.xs),
        ) {
            val s = settings
            if (s == null) {
                KoanText("reading the configuration", KoanRole.Body, KoanTone.Muted)
            } else {
                when (pane) {
                    Pane.Library -> LibraryPane(runtime, s)
                    Pane.Server -> ServerPane(runtime, s)
                    Pane.Playback -> PlaybackPane(runtime, s)
                    Pane.Devices -> DevicesPane(runtime, s)
                    Pane.Appearance -> AppearancePane(runtime)
                }
            }
        }
        StatusLine(error, result)
    }
}

/** What the last action failed with, or came to; empty when neither. */
@Composable
private fun StatusLine(error: String?, result: String?) {
    val text = error ?: result ?: return
    Column(Modifier.fillMaxWidth()) {
        KoanRule()
        KoanText(
            text, KoanRole.Fine, if (error != null) KoanTone.Bad else KoanTone.Muted, data = true,
            modifier = Modifier.padding(horizontal = KoanSpace.page, vertical = KoanSpace.s), maxLines = 3,
        )
    }
}

// -- Library -------------------------------------------------------------------------------------

@Composable
private fun LibraryPane(runtime: KoanRuntime, s: Settings) {
    val model = runtime.settings
    val activity = runtime.activity
    val busy by activity.busy.collectAsStateWithLifecycle()
    var removing by rememberSaveable { mutableStateOf<String?>(null) }
    var rebuilding by rememberSaveable { mutableStateOf(false) }
    val scanning = busy.any { it in Resource.LocalLibrary }

    KoanSectionHeader("folders")
    if (s.libraryFolders.isEmpty()) {
        KoanFootnote(
            "No folders. kōan on Android plays from your server; scanning folders on this phone is not part of this version.",
        )
    }
    for (folder in s.libraryFolders) {
        Column(Modifier.fillMaxWidth().padding(vertical = KoanSpace.xs)) {
            KoanText(folder.path, KoanRole.Meta, data = true)
            Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.m)) {
                KoanText("${folder.tracks} tracks", KoanRole.Fine, KoanTone.Muted, modifier = Modifier.weight(1f))
                KoanButton("remove", { removing = folder.path }, kind = KoanButtonKind.Bordered)
            }
        }
    }
    removing?.let { path ->
        Confirm("stop scanning this folder?", "Removing a folder stops it being scanned. It does not delete anything.") {
            KoanButton("remove and forget its tracks", { model.removeFolder(path, true, activity); removing = null }, kind = KoanButtonKind.Destructive)
            KoanButton("remove, keep them in the library", { model.removeFolder(path, false, activity); removing = null }, kind = KoanButtonKind.Bordered)
            KoanButton("cancel", { removing = null }, kind = KoanButtonKind.Link)
        }
    }

    if (s.libraryFolders.isNotEmpty()) {
        KoanSectionHeader("scan")
        Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.m)) {
            KoanButton("scan", { model.scan(activity) }, kind = KoanButtonKind.Bordered, enabled = !scanning)
            KoanButton("rescan everything", { model.scan(activity, force = true) }, kind = KoanButtonKind.Bordered, enabled = !scanning)
        }
        if (scanning) KoanFootnote("Waiting for the task that is reading your files to finish.")
    }
    ActivityRows(runtime)

    KoanSectionHeader("library")
    KoanFormRow("tracks from the server", value = s.remoteTracks.toString())
    val wholeBusy = busy.any { it in Resource.WholeLibrary }
    if (rebuilding) {
        Confirm(
            "clear the library index?",
            "Forgets every artist, album and track so the next sync builds them again. Favourites survive: they are kept " +
                "against the server's ids. Lyrics, play counts and audio analysis do not.",
        ) {
            KoanButton("clear index", { model.rebuildIndex(activity); rebuilding = false }, kind = KoanButtonKind.Destructive)
            KoanButton("cancel", { rebuilding = false }, kind = KoanButtonKind.Link)
        }
    } else {
        KoanButton("clear library index", { rebuilding = true }, kind = KoanButtonKind.Destructive, enabled = !wholeBusy)
    }
}

/** What koan is busy with, each with a way to stop it where one stops. */
@Composable
private fun ActivityRows(runtime: KoanRuntime) {
    val tasks by runtime.activity.tasks.collectAsStateWithLifecycle()
    if (tasks.isEmpty()) return
    KoanSectionHeader("running")
    for (task in tasks) TaskRow(task) { runtime.activity.cancel(task.id) }
}

@Composable
private fun TaskRow(task: ActivityTask, onCancel: () -> Unit) {
    Row(Modifier.fillMaxWidth(), verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            KoanText(task.label, KoanRole.Body)
            val progress = buildString {
                task.done?.let { append(it); task.total?.let { t -> append(" of ").append(t) }; task.unit?.let { u -> append(' ').append(u) } }
                task.detail?.let { if (isNotEmpty()) append(", "); append(it) }
            }
            if (progress.isNotEmpty()) KoanText(progress, KoanRole.Fine, KoanTone.Muted, data = true)
        }
        if (task.cancellable) KoanButton("cancel", onCancel, kind = KoanButtonKind.Bordered)
    }
}

// -- Server --------------------------------------------------------------------------------------

@Composable
private fun ServerPane(runtime: KoanRuntime, s: Settings) {
    val model = runtime.settings
    val activity = runtime.activity
    val connection by runtime.mirror.connection.collectAsStateWithLifecycle()
    val cacheBytes by runtime.mirror.cacheBytes.collectAsStateWithLifecycle()
    val busy by activity.busy.collectAsStateWithLifecycle()
    var signingOut by rememberSaveable { mutableStateOf(false) }

    if (!s.remoteSignedIn) {
        KoanFootnote("Not signed in to a server.")
        return
    }
    KoanSectionHeader("signed in")
    KoanFormRow("server", value = s.remoteUrl)
    KoanFormRow("user", value = s.remoteUsername)
    KoanFormRow("tracks", value = s.remoteTracks.toString())
    connection?.let { c ->
        val kind = listOfNotNull(c.serverKind, c.serverVersion).joinToString(" ")
        if (kind.isNotEmpty()) KoanFormRow("software", value = kind)
        KoanFormRow("link", value = if (c.linked) "connected" else "not connected")
        if (c.signInRefused) KoanFootnote("Your server refused kōan's sign-in. Sign in again.", tone = KoanTone.Bad)
    }
    Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.m)) {
        // Only the syncs wait on the database writer. Signing out is a config
        // write, and greying it out while a sync runs strands you on a server
        // you are trying to leave.
        KoanButton("sync", { model.syncNow(activity) }, kind = KoanButtonKind.Bordered, enabled = Resource.RemoteTracks !in busy)
        KoanButton("sign out", { signingOut = true }, kind = KoanButtonKind.Destructive)
    }
    if (signingOut) {
        Confirm("sign out of ${SettingsModel.hostOf(s.remoteUrl)}?", "Tracks already synced can stay in the library or go with the account.") {
            KoanButton("sign out and forget its tracks", { model.signOut(true, activity); signingOut = false }, kind = KoanButtonKind.Destructive)
            KoanButton("sign out and keep its tracks", { model.signOut(false, activity); signingOut = false }, kind = KoanButtonKind.Bordered)
            KoanButton("cancel", { signingOut = false }, kind = KoanButtonKind.Link)
        }
    }
    ActivityRows(runtime)

    KoanToggle("offline mode", connection?.offlineManual == true, model::setOffline)
    KoanFootnote("Shows only what is on this phone. It turns on by itself when your server cannot be reached, and off again when it can.")

    KoanSectionHeader("automatic sync")
    KoanToggle("keep the library in sync", s.autoSync, model::setAutoSync)
    if (s.autoSync) {
        KoanPicker(
            "every", SettingsRules.SyncIntervals.map { SettingsRules.intervalLabel(it) to it },
            s.autoSyncIntervalMins.toLong(), model::setAutoSyncInterval,
        )
    }
    KoanFootnote("Each sync asks the server only for what changed since the last.")

    KoanSectionHeader("downloads")
    val workers = s.downloadWorkers.toInt()
    KoanStepper(
        "parallel downloads", workers.toString(),
        { model.setDownloadWorkers(workers - 1) }, { model.setDownloadWorkers(workers + 1) },
        canDecrement = workers > SettingsRules.MIN_WORKERS, canIncrement = workers < SettingsRules.MAX_WORKERS,
    )
    CacheLimitRow(model, s.cacheLimit)
    KoanFormRow("using", value = SettingsRules.bytes(cacheBytes)) {
        KoanButton("clear", { model.clearCache(activity) }, kind = KoanButtonKind.Bordered, enabled = Resource.Downloads !in busy)
    }

    val offered = connection?.extensions.orEmpty()
    if (offered.isNotEmpty()) {
        KoanSectionHeader("the server offers")
        for (ext in offered) {
            KoanText("${ext.name} ${ext.versions.joinToString(", ")}", KoanRole.Fine, KoanTone.Muted, data = true)
        }
    }
}

/**
 * The cache limit, committed whole when typing ends: "5" on the way to "50GB" is
 * not a limit anyone set. A refused value stays in the field with the reason on
 * the status line, so it can be fixed.
 */
@Composable
private fun CacheLimitRow(model: SettingsModel, stored: String) {
    var typed by remember(stored) { mutableStateOf(stored) }
    val commit = { model.setCacheLimit(typed); Unit }
    KoanFormRow("cache limit") {}
    KoanField(
        typed, { typed = it },
        placeholder = "e.g. 50GB, blank for no limit",
        keyboard = KeyboardType.Ascii, ime = ImeAction.Done,
        onDone = commit, onFocusLost = commit,
    )
}

// -- Playback ------------------------------------------------------------------------------------

@Composable
private fun PlaybackPane(runtime: KoanRuntime, s: Settings) {
    val model = runtime.settings
    val bitPerfect by KoanProcess.bitPerfect.collectAsStateWithLifecycle()
    val outputs by runtime.mirror.outputs.collectAsStateWithLifecycle()
    val renderers by runtime.mirror.renderers.collectAsStateWithLifecycle()

    KoanSectionHeader("output")
    KoanFormRow(
        "bit-perfect",
        value = when (val b = bitPerfect) {
            BitPerfectStatus.Exclusive -> "yes, exclusive"
            BitPerfectStatus.MixerBitPerfect -> "yes, mixer"
            is BitPerfectStatus.No -> "no, ${b.reason}"
        },
    )
    KoanFootnote(
        "Bit-perfect output on Android needs a USB DAC on Android 14 or later. The speaker, wired headphones and " +
            "Bluetooth go through the system mixer, which resamples. This is judged from what the device granted, " +
            "not from what was asked for.",
    )
    outputs?.let { o ->
        val choices = buildList {
            add("system default" to OutputChoice.Default)
            o.devices.filter { it.name.isNotEmpty() }.forEach { add(it.name to OutputChoice.Device(it.name)) }
            o.renderers.forEach { add(it.name to OutputChoice.Renderer(it.id)) }
        }
        KoanPicker("play to", choices, o.current, model::selectOutput, keepsCase = true)
    }
    Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.m)) {
        KoanButton("find devices on this network", model::searchRenderers, kind = KoanButtonKind.Bordered)
    }
    if (renderers.renderers.isNotEmpty()) {
        KoanFootnote("${renderers.renderers.size} found: ${renderers.renderers.joinToString { it.name }}")
    }

    KoanSectionHeader("transport")
    KoanToggle("fade on pause", s.fadeOnPause, model::setFadeOnPause)
    KoanFootnote("Pause and resume ramp the volume over a moment instead of cutting.")

    KoanSectionHeader("loudness")
    KoanFormRow("replaygain") {}
    KoanSegmented(
        listOf("off" to "off", "per track" to "track", "per album" to "album"), s.replaygain, model::setReplayGain,
    )
    if (s.replaygain != "off") {
        KoanStepper(
            "pre-amp", SettingsRules.preAmpText(s.preAmpDb),
            { model.setPreAmp(s.preAmpDb - SettingsRules.PRE_AMP_STEP) }, { model.setPreAmp(s.preAmpDb + SettingsRules.PRE_AMP_STEP) },
            canDecrement = s.preAmpDb > SettingsRules.MIN_PRE_AMP, canIncrement = s.preAmpDb < SettingsRules.MAX_PRE_AMP,
        )
    }
    KoanFootnote("Applies the gain written into the file's tags. Per album keeps the relative loudness within a record.")
}

// -- Devices -------------------------------------------------------------------------------------

@Composable
private fun DevicesPane(runtime: KoanRuntime, s: Settings) {
    val model = runtime.settings
    val connection by runtime.mirror.connection.collectAsStateWithLifecycle()
    val devices by runtime.mirror.devices.collectAsStateWithLifecycle()
    val pending by model.pendingPairing.collectAsStateWithLifecycle()
    var code by rememberSaveable { mutableStateOf("") }
    var address by rememberSaveable { mutableStateOf("") }

    if (s.remoteSignedIn && connection?.pairing == true) {
        KoanSectionHeader("pair a device")
        pending?.let { PairingConfirm(it, model) } ?: run {
            val approve = { val typed = code; code = ""; model.offerPairing(typed) }
            KoanField(code, { code = it }, placeholder = "XXXX-XXXX", ime = ImeAction.Go, onDone = approve)
            KoanButton("approve", approve, kind = KoanButtonKind.Prominent, enabled = code.isNotBlank())
        }
        KoanFootnote(
            "A television or another device without a keyboard shows a code while it waits. Enter it here to sign it in " +
                "as you, with a key of its own that can be revoked on the server.",
        )
    }

    KoanSectionHeader("this device")
    connection?.let { KoanFormRow("name", value = it.thisDevice) }
    KoanToggle("discoverable on this network", s.devicesDiscoverable, model::setDiscoverable)
    KoanFormRow("devices on this network") {}
    KoanSegmented(listOf("full control" to "full", "playback only" to "playback"), s.devicesNearbyControl, model::setNearbyControl)
    connection?.listeningPort?.let { KoanFormRow("listening on port", value = it.toString()) }
    if (connection?.localNetworkBlocked == true) KoanFootnote("This app is not allowed onto the local network.", tone = KoanTone.Bad)
    KoanFootnote(
        "Other kōan apps on this network can see and control what plays here. Full control adds the output, preset, " +
            "volume and moving the music; playback only is play and the queue. Neither reaches your library. On a " +
            "network shared with strangers, choose playback only.",
    )

    val connections = connection?.connections.orEmpty()
    val held = connection?.held.orEmpty()
    if (connections.isNotEmpty() || held.isNotEmpty() || s.devicesRefused.isNotEmpty()) {
        KoanSectionHeader("connected now")
        connections.forEachIndexed { i, c -> ConnectionRow(c, model, i) }
        for (h in held) {
            DeviceLine(h.name ?: h.addr, "disconnected until allowed back") {
                KoanButton("allow", { model.allowHeld(h.addr) }, kind = KoanButtonKind.Bordered)
            }
        }
        for (addr in s.devicesRefused) {
            DeviceLine(addr, "refused") {
                KoanButton("allow", { model.allowRefused(addr) }, kind = KoanButtonKind.Bordered)
            }
        }
        KoanFootnote(
            "Disconnect hangs up a device on this network until you allow it back here. Refuse keeps a device that has " +
                "not proved it is yours out for good, by its address. Your account's devices reach this one through the " +
                "server for as long as they are signed in.",
        )
    }

    KoanSectionHeader("devices by address")
    for (addr in s.devicesAddresses) {
        DeviceLine(addr, null) { KoanButton("remove", { model.removeAddress(addr) }, kind = KoanButtonKind.Bordered) }
    }
    val add = { if (model.addAddress(address)) address = "" }
    KoanField(address, { address = it }, placeholder = "host or host:port", keyboard = KeyboardType.Uri, ime = ImeAction.Done, onDone = add)
    KoanButton("add", add, kind = KoanButtonKind.Bordered, enabled = address.isNotBlank())
    KoanFootnote("For networks that do not announce devices, such as a tailnet. The port is 5626 unless the other device says otherwise.")

    if (devices.devices.isNotEmpty()) {
        KoanSectionHeader("your devices")
        for (d in devices.devices) {
            val how = listOfNotNull(
                d.platform, if (d.nearby) "nearby" else null, if (d.account) "account" else null,
                d.owner?.let { "shared by $it" }, if (d.asleep) "asleep" else null,
            ).joinToString(", ")
            DeviceLine(d.name, how, nameIsData = true)
        }
    }
}

@Composable
private fun ConnectionRow(c: ConnectedInfo, model: SettingsModel, index: Int) {
    val how = listOfNotNull(
        if (c.viaServer) "through the server" else "on this network",
        if (c.inbound) "can control this device" else "controlled from here",
        if (c.own) "yours" else c.owner?.let { "shared by $it" },
    ).joinToString(", ")
    DeviceLine(c.name, how, key = index) {
        if (c.key != null) {
            if (!c.own && c.owner == null && c.addr != null) {
                KoanButton("refuse", { model.endConnection(c, refuse = true) }, kind = KoanButtonKind.Destructive)
            }
            KoanButton("disconnect", { model.endConnection(c, refuse = false) }, kind = KoanButtonKind.Bordered)
        }
    }
}

@Composable
private fun DeviceLine(
    name: String,
    detail: String?,
    @Suppress("UNUSED_PARAMETER") key: Int = 0,
    nameIsData: Boolean = true,
    actions: (@Composable () -> Unit)? = null,
) {
    Row(
        Modifier.fillMaxWidth(),
        verticalAlignment = androidx.compose.ui.Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(KoanSpace.s),
    ) {
        Column(Modifier.weight(1f).padding(vertical = KoanSpace.xs)) {
            KoanText(name, KoanRole.Body, data = nameIsData)
            if (detail != null) KoanText(detail, KoanRole.Fine, KoanTone.Muted, maxLines = 2)
        }
        actions?.invoke()
    }
}

/** "Sign in this device?", for a pairing link or a typed code. */
@Composable
private fun PairingConfirm(request: PairingRequest, model: SettingsModel) {
    Confirm(
        "sign in ${request.device}?",
        "${request.origin}\n\nIt will be signed in as ${request.username} on ${request.host}. Allow only a device you are " +
            "setting up yourself: anyone can give a device any name.",
        titleIsData = true,
    ) {
        KoanButton("allow", { model.settlePairing(true) }, kind = KoanButtonKind.Prominent)
        KoanButton("decline", { model.settlePairing(false) }, kind = KoanButtonKind.Link)
    }
}

// -- Appearance ----------------------------------------------------------------------------------

@Composable
private fun AppearancePane(runtime: KoanRuntime) {
    val model = runtime.settings
    val appearance by model.appearance.collectAsStateWithLifecycle()
    val a = appearance
    KoanSectionHeader("colour")
    if (a == null) {
        KoanText("reading the configuration", KoanRole.Body, KoanTone.Muted)
        return
    }
    KoanToggle("colours from the record", a.recordColours, model::setRecordColours)
    KoanFootnote(
        "The record playing sets the accent on selection and progress. Off, the accent is kōan's mint. " +
            "Light and dark follow the system.",
    )
}

// -- Shared --------------------------------------------------------------------------------------

/** A question put in the page, in a one-point rule above and below: the theme has no dialog, and nothing here needs one. */
@Composable
private fun Confirm(
    title: String,
    body: String,
    titleIsData: Boolean = false,
    actions: @Composable ColumnScope.() -> Unit,
) {
    Column(Modifier.fillMaxWidth().padding(vertical = KoanSpace.s), verticalArrangement = Arrangement.spacedBy(KoanSpace.xs)) {
        KoanRule()
        KoanText(title, KoanRole.Body, KoanTone.Strong, data = titleIsData, maxLines = 2)
        KoanFootnote(body)
        actions()
        KoanRule()
    }
}
