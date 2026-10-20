package cc.blit.koan.ui.settings

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.material3.AlertDialog
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboard
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.ffi.DeletedDsp
import cc.blit.koan.ffi.DspOverview
import cc.blit.koan.ffi.DspProfileSummary
import cc.blit.koan.ffi.DspRole
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.navigation.nonce
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanMenu
import cc.blit.koan.ui.theme.KoanMenuItem
import cc.blit.koan.ui.theme.KoanSectionHeader
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import kotlinx.coroutines.launch
import kotlinx.serialization.Serializable

/** Every correction, EQ and preset; `device` is the one the EQ page shows, which Use and Add act on. */
@Serializable
data class ManageEqRoute(val device: String?, val nonce: String = nonce()) : NavKey

/** The server keeps deleted EQs for thirty days, and restores them. */
private const val DSP_DELETED = "koanDspDeleted"

/**
 * Every correction, EQ and preset, where each is used, and what can be done
 * to it: open, use, rename, copy, revert, keep on every device, delete.
 * Groups are headings, their members under them. Reached from the EQ page,
 * which shows one device; this shows them all.
 */
@Composable
fun ManageEqPage(device: String?, model: SettingsModel) {
    val app = koanApp()
    val dsp = dspModel()
    val nav = LocalNavigator.current
    val context = LocalContext.current
    val clipboard = LocalClipboard.current
    val scope = rememberCoroutineScope()
    val settings by model.state.collectAsStateWithLifecycle()
    val state by dsp.state.collectAsStateWithLifecycle()
    val library by app.mirror.library.collectAsStateWithLifecycle()
    val connection by app.mirror.connection.collectAsStateWithLifecycle()
    var renaming by remember { mutableStateOf<String?>(null) }
    var copying by remember { mutableStateOf<String?>(null) }
    var deleting by remember { mutableStateOf<String?>(null) }
    val shown by produceState<DspOverview?>(null, device, state.version, library) { value = dsp.overviewFor(device) }
    val offered = connection.offers(DSP_DELETED) && connection?.offline != true
    // Read again only when something here is deleted or restored. A failed read is not an empty one: nothing is said until one answers.
    var deletions by remember { mutableIntStateOf(0) }
    val deleted by produceState<List<DeletedDsp>?>(null, offered, deletions) { value = if (offered) dsp.deleted() else null }
    var restoring by remember { mutableStateOf<String?>(null) }
    var restoreError by remember { mutableStateOf<String?>(null) }
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        if (uris.isNotEmpty()) scope.launch { dsp.importPaths(copyIn(context, uris)) }
    }
    val open = { name: String -> nav.push(DspProfileRoute(name, device)) }

    SettingsPage("Manage EQ", settings) {
        shown?.let { o ->
            val actions = ProfileActions(o, device, dsp, open, rename = { renaming = it }, copy = { copying = it }, delete = { deleting = it })
            val grouped = o.profiles.flatMap { it.members }.toSet()
            val loose = o.profiles.filter { !it.preset && it.members.isEmpty() && it.name !in grouped }
            profiles("Corrections", loose.filter { it.role != DspRole.TUNING }, actions)
            profiles("EQs", loose.filter { it.role == DspRole.TUNING }, actions)
            o.profiles.filter { it.members.isNotEmpty() }.forEach { group ->
                item("group-${group.name}") {
                    Row(inset.fillMaxWidth().padding(top = KoanSpace.l), verticalAlignment = Alignment.CenterVertically) {
                        KoanSectionHeader(group.name, Modifier.weight(1f))
                        if (device != null && group.role == DspRole.TUNING) {
                            KoanButton("Add all to filters", { actions.addToFilters(group.members) }, kind = KoanButtonKind.Link)
                        }
                        ActionsMenu(group, actions)
                    }
                }
                group.members.mapNotNull { name -> o.profiles.firstOrNull { it.name == name } }.forEach { p ->
                    item("member-${group.name}-${p.name}") { ProfileRow(p, actions) }
                }
            }
            profiles("Presets", o.profiles.filter { it.preset }, actions)
        }

        deleted?.let { gone ->
            header("Recently deleted")
            if (gone.isEmpty()) line("Nothing deleted in the last thirty days", KoanTone.Muted)
            gone.forEach { profile ->
                item("deleted-${profile.uid}") {
                    Row(inset.fillMaxWidth().padding(vertical = KoanSpace.xs), verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            KoanText(profile.name)
                            KoanText(if (profile.daysLeft == 1u) "1 day left" else "${profile.daysLeft} days left", role = KoanType.Fine, tone = KoanTone.Muted)
                        }
                        KoanButton("Restore", {
                            restoring = profile.uid
                            scope.launch {
                                restoreError = dsp.restore(profile.uid)
                                restoring = null
                                deletions++
                            }
                        }, kind = KoanButtonKind.Bordered, enabled = restoring == null)
                    }
                }
            }
            footer(restoreError ?: "Kept on the server for thirty days after they were deleted. Restoring one brings it back on every device.")
        }

        header("Add")
        item {
            ButtonRow {
                KoanButton("Import a file…", { pick.launch(arrayOf("*/*")) }, kind = KoanButtonKind.Bordered)
                KoanButton("Find in AutoEQ…", { nav.push(AutoEqRoute()) }, kind = KoanButtonKind.Bordered)
            }
        }
        item {
            ButtonRow {
                KoanButton("Find a measurement…", { nav.push(MeasurementRoute()) }, kind = KoanButtonKind.Bordered)
                KoanButton("Paste EQ text", {
                    scope.launch {
                        clipboard.getClipEntry()?.clipData?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.text?.toString()?.let(dsp::importText)
                    }
                }, kind = KoanButtonKind.Bordered)
            }
        }
        state.summary?.let { line(it, KoanTone.Muted) }
        state.error?.let { line(it, KoanTone.Bad) }
        footer(
            "AutoEQ and Equalizer APO text, impulse WAVs, Roon zips, Convolver .cfg and CamillaDSP configs, or a headphone found in AutoEQ by " +
                "name. Importing under a name already used adds to that EQ; several files at once become a group. Files shared to kōan, " +
                "or opened with it, are imported the same way.",
        )
    }

    renaming?.let { old ->
        NameDialog("Rename", old, "Rename", { renaming = null }) { new -> dsp.rename(old, new) }
    }
    copying?.let { name ->
        NameDialog("Save as new", "$name copy", "Save", { copying = null }, message = "A copy as it is now, to change apart from the original.") { new ->
            dsp.duplicate(name, new)
        }
    }
    deleting?.let { name ->
        val p = shown?.profiles?.firstOrNull { it.name == name }
        AlertDialog(
            onDismissRequest = { deleting = null },
            title = { KoanText("Delete $name?", role = KoanType.TitleSmall) },
            text = { KoanText(p?.let { deleteMessage(it, shown) }.orEmpty()) },
            confirmButton = {
                KoanButton("Delete", {
                    deleting = null
                    dsp.delete(name) { deletions++ }
                }, destructive = true)
            },
            dismissButton = { KoanButton("Cancel", { deleting = null }) },
        )
    }
}

/** Where a deleted profile goes from: the devices that play it, and the presets and groups that hold it, which go on without it. */
internal fun deleteMessage(p: DspProfileSummary, o: DspOverview?): String {
    val lines = listOfNotNull(
        p.usedOn.takeIf { it.isNotEmpty() }?.let { "Used on ${listed(it.map { d -> o?.names?.get(d) ?: d })}, which will play without it." },
        p.heldBy.takeIf { it.isNotEmpty() }?.let { "It is taken out of ${listed(it)}." },
    )
    return if (lines.isEmpty()) "It is not used anywhere." else lines.joinToString(" ")
}

/** What can be done to a profile from Manage EQ, on the device the EQ page shows. */
private class ProfileActions(
    val o: DspOverview,
    val device: String?,
    val dsp: DspModel,
    val open: (String) -> Unit,
    val rename: (String) -> Unit,
    val copy: (String) -> Unit,
    val delete: (String) -> Unit,
) {
    fun label(d: String) = o.names[d] ?: d

    fun addToFilters(names: List<String>) {
        val device = device ?: return
        dsp.setTunings(names.fold(o.chain) { chain, name -> chain.added(name) }, device)
    }
}

private fun LazyListScope.profiles(title: String, profiles: List<DspProfileSummary>, actions: ProfileActions) {
    if (profiles.isEmpty()) return
    header(title)
    profiles.forEach { p -> item("$title-${p.name}") { ProfileRow(p, actions) } }
}

@Composable
private fun ProfileRow(p: DspProfileSummary, actions: ProfileActions) {
    Row(inset.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        KoanButton({ actions.open(p.name) }, Modifier.weight(1f), kind = KoanButtonKind.Card) {
            Column(Modifier.fillMaxWidth().padding(vertical = KoanSpace.xs), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.s), verticalAlignment = Alignment.CenterVertically) {
                    KoanText(p.name, Modifier.weight(1f, fill = false))
                    if (p.edited) KoanText("Edited", role = KoanType.Fine, tone = KoanTone.Muted, appText = true)
                }
                val problem = p.problem
                if (problem != null) {
                    KoanText(problem, role = KoanType.Fine, tone = KoanTone.Bad)
                } else {
                    val used = if (p.usedOn.isEmpty()) "Not used" else "Used on ${listed(p.usedOn.map(actions::label))}"
                    KoanText(listOf(DspModel.describe(p), used).filter { it.isNotEmpty() }.joinToString(" · "), role = KoanType.Fine, tone = KoanTone.Muted)
                }
            }
        }
        ActionsMenu(p, actions)
    }
}

@Composable
private fun ActionsMenu(p: DspProfileSummary, actions: ProfileActions) {
    var open by remember { mutableStateOf(false) }
    val dsp = actions.dsp
    val o = actions.o
    val device = actions.device
    fun then(work: () -> Unit) {
        open = false
        work()
    }
    Box {
        KoanButton("Options for ${p.name}", { open = true }, kind = KoanButtonKind.Icon, icon = KoanIcons.more)
        KoanMenu(open, { open = false }) {
            KoanMenuItem("Open", KoanIcons.disclosure) { then { actions.open(p.name) } }
            if (device != null) {
                when {
                    p.preset -> KoanMenuItem("Use on ${actions.label(device)}") { then { dsp.applyPreset(p.name, device) } }
                    p.role == DspRole.TUNING && p.rates.isEmpty() && o.chain.none { it.name == p.name } ->
                        KoanMenuItem("Add to filters", KoanIcons.add) { then { actions.addToFilters(listOf(p.name)) } }
                    p.role != DspRole.TUNING && o.active != p.name ->
                        KoanMenuItem("Use as correction") { then { dsp.assign(p.name, device) } }
                }
            }
            KoanMenuItem("Rename…") { then { actions.rename(p.name) } }
            KoanMenuItem("Save as new…", KoanIcons.copy) { then { actions.copy(p.name) } }
            if (p.edited) KoanMenuItem("Revert to imported", KoanIcons.undo) { then { dsp.revert(p.name) } }
            val locked = p.scopeLocked
            if (locked != null) {
                KoanMenuItem("${if (p.everywhere) "Kept on every device" else "Kept on this device"}: $locked", KoanIcons.sync, enabled = false) {}
            } else {
                KoanMenuItem(if (p.everywhere) "Keep on this device only" else "Keep on every device", KoanIcons.sync) {
                    then { dsp.setScope(p.name, !p.everywhere) }
                }
            }
            KoanMenuItem("Delete…", KoanIcons.clear, destructive = true) { then { actions.delete(p.name) } }
        }
    }
}
