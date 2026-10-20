package cc.blit.koan.ui.settings

import android.os.SystemClock
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.ffi.DspEarKind
import cc.blit.koan.ffi.DspImpulse
import cc.blit.koan.ffi.DspLayerInfo
import cc.blit.koan.ffi.DspPoint
import cc.blit.koan.ffi.DspProfileDetail
import cc.blit.koan.ffi.DspProfileSummary
import cc.blit.koan.ffi.DspResponse
import cc.blit.koan.ffi.DspRole
import cc.blit.koan.ffi.DspTargetName
import cc.blit.koan.ffi.DspTargets
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.navigation.nonce
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanField
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanMenu
import cc.blit.koan.ui.theme.KoanMenuItem
import cc.blit.koan.ui.theme.KoanPicker
import cc.blit.koan.ui.theme.KoanSegmented
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanToggle
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import kotlin.math.abs
import kotlin.math.hypot
import kotlinx.coroutines.launch
import kotlinx.serialization.Serializable

/** One EQ or convolution profile's page; `device` is the output whose tuning a copy saved there takes its place in, the one in use if none. */
@Serializable
data class DspProfileRoute(val name: String, val device: String? = null, val nonce: String = nonce()) : NavKey

/**
 * A profile, and exactly what is in it: what it is for and the target it
 * corrects to, the profiles it plays first, its bands drawn and editable,
 * its impulse responses and headroom, where it is kept, and the outputs that
 * play it. Renamed in place; deleted from here.
 */
@Composable
fun DspProfilePage(initial: String, device: String?, model: SettingsModel) {
    val app = koanApp()
    val dsp = dspModel()
    val nav = LocalNavigator.current
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val settings by model.state.collectAsStateWithLifecycle()
    val state by dsp.state.collectAsStateWithLifecycle()
    val library by app.mirror.library.collectAsStateWithLifecycle()
    var name by rememberSaveable { mutableStateOf(initial) }
    var editingName by rememberSaveable(name) { mutableStateOf(name) }
    var detail by remember { mutableStateOf<DspProfileDetail?>(null) }
    var response by remember { mutableStateOf<DspResponse?>(null) }
    var targets by remember { mutableStateOf<DspTargets?>(null) }
    var madeForChoices by remember { mutableStateOf(TargetGroups()) }
    // For a tuning: the target it looks made against while it does not say, and what it adds on the output's correction for each choice.
    var suggestion by remember { mutableStateOf<DspTargetName?>(null) }
    var previews by remember { mutableStateOf(emptyMap<String, List<Double>>()) }
    // The EQ as it was when the page opened, which Save as copy puts back, and as it is now.
    var before by remember { mutableStateOf<String?>(null) }
    var now by remember { mutableStateOf<String?>(null) }
    // Edits made here when `before` was taken: until there are more, a change is another device's, and `before` follows it.
    var editsAtBefore by remember { mutableStateOf<Int?>(null) }
    // What became of the last Save as copy, where it needs saying.
    var notice by remember { mutableStateOf<String?>(null) }
    var showingMore by rememberSaveable { mutableStateOf(false) }
    var copying by remember { mutableStateOf(false) }
    var resetting by remember { mutableStateOf(false) }
    var deleting by remember { mutableStateOf(false) }
    var askingKeep by remember { mutableStateOf(false) }
    // The channel whose bands the graph and table show, in stereo.
    var channel by rememberSaveable(name) { mutableStateOf(0) }
    val addTarget = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) scope.launch { copyIn(context, listOf(uri)).firstOrNull()?.let(dsp::addTarget) }
    }
    // Read again whenever a change lands, here or synced.
    LaunchedEffect(name, state.version, library) {
        val snapshot = dsp.snapshot(name)
        now = snapshot
        val edits = dsp.edits(name)
        if (before == null || editsAtBefore == edits) {
            before = snapshot
            editsAtBefore = edits
        }
        val d = dsp.detail(name)
        detail = d
        response = dsp.response(name)
        targets = dsp.targets(name)
        madeForChoices = TargetGroups.of(dsp.targetsFor(DspEarKind.OVER_EAR), dsp.targetsFor(DspEarKind.IN_EAR), state.overview?.inEar)
        if (d?.role == DspRole.TUNING) {
            suggestion = dsp.suggestMadeAgainst(name)
            previews = dsp.madeAgainstPreviews(name)
        } else {
            suggestion = null
            previews = emptyMap()
        }
    }
    val inUse = state.overview?.device
    val label = { d: String -> state.overview?.names?.get(d) ?: d }

    SettingsPage(name, settings) {
        val d = detail ?: return@SettingsPage
        val r = response
        // Stereo needs bands to copy: an empty EQ has no channels to split.
        val layout = d.channels?.takeIf { !d.readOnly && d.bands.isNotEmpty() }
        val editing = if (layout?.stereo == true) channel else null
        val ink = KoanTheme.color(KoanTone.Ink)
        if (r != null) {
            // A tuning as it plays on the output's correction, where that differs from the tuning alone: with the
            // target difference its Made against asks for, so a wrong choice shows as a preference doubled or taken out.
            val onCorrection = previews[d.tunedFor ?: ""]?.takeIf { it.size == r.total.size && it != r.total }
            item { EqEditor(dsp, name, d, r, editing, onCorrection?.let { Curve(it, ink, 1.2f, dashed = true, pattern = StageStroke.eq(1).pattern!!) }) }
        }
        item { ChainSummaryCard(d.corrects, d.correctsBaked, d.tunings, inset.padding(vertical = KoanSpace.s)) }
        d.correctsTwice?.let { line(it, KoanTone.Bad) }
        item {
            KoanField(
                editingName,
                { editingName = it },
                inset.fillMaxWidth().padding(vertical = KoanSpace.s).onFocusChanged { if (!it.isFocused) editingName = name },
                placeholder = "Name",
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
                keyboardActions = KeyboardActions(onDone = {
                    val new = editingName.trim()
                    if (new.isNotEmpty() && new != name) dsp.rename(name, new) { ok -> if (ok) name = new else editingName = name }
                }),
            )
        }
        (notice ?: state.error)?.let { line(it, KoanTone.Bad) }

        layout?.let { l ->
            header("Channels")
            item {
                KoanSegmented(listOf("Mono" to false, "Stereo" to true), l.stereo, { stereo ->
                    if (stereo != l.stereo) {
                        if (!stereo && l.differ) {
                            askingKeep = true
                        } else {
                            dsp.setStereo(name, stereo)
                            channel = 0
                        }
                    }
                }, inset)
            }
            if (l.stereo) item { KoanSegmented(listOf("Left" to 0, "Right" to 1), channel, { channel = it }, inset) }
            footer(
                if (l.stereo) {
                    "Each channel has bands of its own. A mono recording plays the left channel's; an output with more than " +
                        "two channels has them on its front left and right alone."
                } else {
                    "One list of bands for both channels. Stereo starts each channel with a copy of it."
                },
            )
        }

        // Keeping an edit apart from what was there: as a copy, with this EQ put back, or for an import, going back to its file.
        if (!d.readOnly && (before != now || (d.canRevert && d.edited))) {
            item {
                ButtonRow {
                    KoanButton("Save as copy…", { copying = true }, kind = KoanButtonKind.Bordered)
                    if (d.canRevert && d.edited) KoanButton("Reset to file", { resetting = true }, kind = KoanButtonKind.Bordered)
                }
            }
            footer(
                if (d.canRevert) {
                    "Save as copy keeps this edit as an EQ of its own, in this one's place in the filters, and puts this one back. " +
                        "Reset to file puts it back as ${d.source.firstOrNull() ?: "its file"} had it."
                } else {
                    "Save as copy keeps this edit as an EQ of its own, in this one's place in the filters, and puts this one back as it " +
                        "was when you opened it."
                },
            )
        }
        d.problem?.let { line(it, KoanTone.Bad) }

        header("Used for")
        if (d.devices.isEmpty()) line(KoanTheme.label("No output yet"), KoanTone.Muted)
        d.devices.forEach { line(label(it)) }
        inUse?.let { output ->
            item {
                ButtonRow {
                    if (output in d.devices) {
                        KoanButton({ dsp.use(null) }, kind = KoanButtonKind.Bordered) { KoanText("${KoanTheme.label("Stop using for")} ${label(output)}", role = KoanType.Meta) }
                    } else {
                        KoanButton({ dsp.use(d.name) }, kind = KoanButtonKind.Bordered) { KoanText("${KoanTheme.label("Use for")} ${label(output)}", role = KoanType.Meta) }
                    }
                }
            }
        }

        // A stack with nothing of its own is what its layers are.
        if (d.layers.isEmpty() || d.bands.isNotEmpty() || d.impulses.isNotEmpty()) {
            roleSection(dsp, d, madeForChoices, targets, suggestion, previews, addTarget = { addTarget.launch(arrayOf("*/*")) }) {
                nav.push(SplitRoute(name))
            }
        }
        if (d.group) groupSection(dsp, d) else layersSection(dsp, d, state.overview?.profiles.orEmpty())

        // A correction is finished as installed; what it is made of is there for those who look. A tuning is its bands.
        if (d.role != DspRole.TUNING) {
            item {
                ButtonRow {
                    KoanButton(if (showingMore) "Less" else "Bands, sync and more", { showingMore = !showingMore }, kind = KoanButtonKind.Link)
                }
            }
        }
        if (d.role == DspRole.TUNING || showingMore) {
            scopeSection(dsp, d)
            if (d.impulses.isNotEmpty()) {
                header("Impulse responses")
                d.impulses.forEach { i -> item("impulse-${i.file}") { ImpulseRow(i) } }
                footer("A track at a rate with no response of its own is resampled to the nearest one here.")
            }
            bandTable(dsp, name, d, editing)
            header("Headroom")
            item { ValueRow("Preamp", "%.1f dB".format(d.preampDb)) }
            footer(
                if (d.preampSet) "Set in this EQ."
                else "Derived at ${DspModel.khz(d.preampRate)} kHz from the largest gain the filters apply, so nothing they boost can clip.",
            )
            if (d.source.isNotEmpty()) {
                header("Imported from")
                d.source.forEach { file -> line(file, KoanTone.Muted) }
            }
        }
        item { ButtonRow { KoanButton("Delete", { deleting = true }, kind = KoanButtonKind.Bordered, destructive = true) } }
    }

    if (copying) {
        NameDialog(
            "Save as copy",
            "$name copy",
            "Save",
            { copying = false },
            message = "The copy takes this EQ's place in the filters, and this one goes back to how it was.",
            taken = { new -> "There is already an EQ called $new.".takeIf { state.overview?.profiles?.any { it.name == new } == true } },
        ) { new ->
            val (from, kept, on) = Triple(name, before, device ?: inUse)
            scope.launch {
                val copy = dsp.saveAsCopy(from, new, kept, on) ?: return@launch
                notice = if (copy.placed) null
                else "${copy.name} is saved, but $from is not in ${on?.let(label) ?: "the output"}'s filters, so the copy is not either. Add it on the EQ page."
                before = null
                name = copy.name
            }
        }
    }
    if (resetting) {
        AlertDialog(
            onDismissRequest = { resetting = false },
            title = { KoanText("Reset $name to its file?", role = KoanType.TitleSmall) },
            text = { KoanText("Its edits are undone, and it plays as ${detail?.source?.firstOrNull() ?: "its file"} had it.") },
            confirmButton = {
                KoanButton("Reset", {
                    resetting = false
                    dsp.revert(name)
                    before = null
                }, destructive = true)
            },
            dismissButton = { KoanButton("Cancel", { resetting = false }) },
        )
    }
    if (askingKeep) {
        AlertDialog(
            onDismissRequest = { askingKeep = false },
            title = { KoanText("Keep which channel?", role = KoanType.TitleSmall) },
            text = { KoanText("The two channels have different bands. One list plays on both, and the other channel's bands are dropped.") },
            confirmButton = {
                Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.s)) {
                    listOf("Keep left" to 0, "Keep right" to 1).forEach { (label, keep) ->
                        KoanButton(label, {
                            askingKeep = false
                            dsp.setStereo(name, false, keep)
                            channel = 0
                        })
                    }
                }
            },
            dismissButton = { KoanButton("Cancel", { askingKeep = false }) },
        )
    }
    if (deleting) {
        val summary = state.overview?.profiles?.firstOrNull { it.name == name }
        AlertDialog(
            onDismissRequest = { deleting = false },
            title = { KoanText("Delete $name?", role = KoanType.TitleSmall) },
            text = { KoanText(summary?.let { deleteMessage(it, state.overview) } ?: "Its impulse responses are deleted with it.") },
            confirmButton = {
                KoanButton("Delete", {
                    deleting = false
                    dsp.delete(name)
                    nav.pop()
                }, destructive = true)
            },
            dismissButton = { KoanButton("Cancel", { deleting = false }) },
        )
    }
}

/**
 * What a profile is for, and for a correction, the target it corrects to:
 * the target belongs to the correction, whose job is to make the headphone
 * neutral, and anything more is said against neutral. A tuning says what it
 * was made against, each choice drawn as what it adds on the correction in
 * use.
 */
private fun LazyListScope.roleSection(
    dsp: DspModel,
    d: DspProfileDetail,
    madeForChoices: TargetGroups,
    targets: DspTargets?,
    suggestion: DspTargetName?,
    previews: Map<String, List<Double>>,
    addTarget: () -> Unit,
    split: () -> Unit,
) {
    header("What it's for")
    item { ChoiceRow("This is", d.role, listOf(DspRole.CORRECTION, DspRole.BAKED, DspRole.TUNING).map { it.label to it }, { dsp.setRole(d.name, it) }) }
    val unknown = { id: String -> madeForChoices.option(id)?.name ?: "Unknown" }
    val does = { id: String -> madeForChoices.option(id)?.does.orEmpty() }
    if (d.role == DspRole.TUNING && !madeForChoices.isEmpty) {
        // One scale for every choice's curve, so a doubled shelf stands taller.
        val range = maxOf(6.0, previews.values.flatten().maxOfOrNull { abs(it) } ?: 0.0)
        item {
            TargetPickerRow("Made against", d.tunedFor.orEmpty(), madeForChoices.sections, unknown, { dsp.setTunedFor(d.name, it.ifEmpty { null }) }, does) { id ->
                previews[id]?.let { CurveThumb(it, StageStroke.eq(1), range) }
            }
        }
        if (d.tunedFor == null && suggestion != null) {
            item {
                ButtonRow {
                    KoanButton("Looks made for ${suggestion.name}. Use that?", { dsp.setTunedFor(d.name, suggestion.id) }, kind = KoanButtonKind.Link)
                }
            }
        }
    }
    if (d.role == DspRole.BAKED && d.impulses.isEmpty() && d.layers.isEmpty()) {
        item { ButtonRow { KoanButton("Split into correction + tuning…", split, kind = KoanButtonKind.Bordered) } }
    }
    if (d.role == DspRole.CORRECTION) {
        if (targets != null) {
            val madeFor = targets.madeFor?.id
            val current = targets.chosen ?: madeFor ?: ""
            item {
                TargetPickerRow(
                    "Corrected to",
                    current,
                    listOf(null to targets.choices.map { it.id }),
                    name = { id -> targets.choices.firstOrNull { it.id == id }?.name.orEmpty() },
                    onSelect = { id -> dsp.chooseTarget(d.name, id.takeUnless { it == madeFor }) },
                    describe = { id -> targets.choices.firstOrNull { it.id == id }?.does.orEmpty() },
                )
            }
            targets.choices.firstOrNull { it.id == current }?.character?.takeIf { it.isNotEmpty() }?.let { line(it, KoanTone.Muted) }
            item { ButtonRow { KoanButton("Add a target…", addTarget, kind = KoanButtonKind.Bordered) } }
        }
        if (targets == null && !d.measured && !madeForChoices.isEmpty) {
            item {
                TargetPickerRow("Made for", d.madeFor.orEmpty(), madeForChoices.sections, unknown, { dsp.setMadeFor(d.name, it.ifEmpty { null }) }, does)
            }
        }
    }
    footer(roleFooter(d, targets))
}

private fun roleFooter(d: DspProfileDetail, targets: DspTargets?) = when {
    d.role == DspRole.TUNING ->
        "A tuning is taste: more bass, a darker treble. It plays on top of a correction. Say which target it was made against, and on a " +
            "device corrected to another, kōan plays the difference first, so it sounds as it was made to."
    d.role == DspRole.BAKED ->
        "A correction with a tuning already in it, as most finished presets are. It is the device's correction, so another tuning on top " +
            "would add taste twice. Split it, with a measurement of the device, to change the tuning."
    d.measured ->
        "A correction makes your headphones or speakers neutral, and the target says what neutral is. This one is worked out again from " +
            "the measurement for each target."
    targets?.madeFor != null ->
        "Made for ${targets.madeFor!!.name}. Another target is worked out from the measurement AutoEQ kept, where there is one, or plays as " +
            "the difference between the two. Moving from Harman to neutral takes Harman's bass and treble out."
    else ->
        "A correction makes your headphones or speakers neutral. Say which target this EQ was made for, and you can move it to another; " +
            "if you don't know, leave it Unknown and target switching stays off."
}

/** A group's members, one playing, chosen as a radio button is. */
private fun LazyListScope.groupSection(dsp: DspModel, d: DspProfileDetail) {
    header("Group: pick one")
    val playing = d.layers.firstOrNull { it.on }?.profile ?: d.layers.firstOrNull()?.profile
    d.layers.forEach { layer ->
        item("member-${layer.profile}") {
            KoanButton({ dsp.select(d.name, layer.profile) }, inset, kind = KoanButtonKind.Card) {
                Row(Modifier.fillMaxWidth().padding(vertical = KoanSpace.s), verticalAlignment = Alignment.CenterVertically) {
                    KoanText(layer.profile, Modifier.weight(1f), tone = if (layer.profile == playing) KoanTone.Accent else KoanTone.Ink)
                    if (layer.profile == playing) KoanIcon(KoanIcons.check, contentDescription = "Playing", color = KoanTheme.color(KoanTone.Accent))
                }
            }
        }
    }
    item { ButtonRow { KoanButton("Play them all in order", { dsp.setGroup(d.name, false) }, kind = KoanButtonKind.Bordered) } }
    footer(
        if (d.layers.any { it.on }) {
            "One member plays at a time. Pick another and it plays in place of the last. Each member is an EQ of its own, with its own page."
        } else {
            "None was picked, so the first plays. Pick one to change it."
        },
    )
}

/**
 * The profiles a stack plays first, in order, each switched on or off: a
 * headphone's correction, then taste on top of it. Any profile can become a
 * stack; one with impulse responses cannot be a layer. Removing one takes it
 * out of this list and keeps the EQ.
 */
private fun LazyListScope.layersSection(dsp: DspModel, d: DspProfileDetail, profiles: List<DspProfileSummary>) {
    val layers = d.layers
    val addable = profiles.filter { p -> p.name != d.name && p.rates.isEmpty() && layers.none { it.profile == p.name } }
    if (layers.isEmpty()) {
        // Nothing on top: a quiet offer, not an empty section.
        if (addable.isNotEmpty()) item { ButtonRow { AddLayerMenu("Add filters…", addable) { dsp.setLayers(d.name, layers + DspLayerInfo(it, true)) } } }
        return
    }
    header("Plays first")
    layers.forEachIndexed { i, layer ->
        item("layer-${layer.profile}") {
            var menu by remember { mutableStateOf(false) }
            Row(inset.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                d.layerRoles.getOrNull(i)?.let { RoleTag(it) }
                KoanToggle(layer.profile, layer.on, { on ->
                    dsp.setLayers(d.name, layers.mapIndexed { j, l -> if (j == i) DspLayerInfo(l.profile, on) else l })
                }, Modifier.weight(1f).padding(start = KoanSpace.s))
                Box {
                    KoanButton("Options for ${layer.profile}", { menu = true }, kind = KoanButtonKind.Icon, icon = KoanIcons.more)
                    KoanMenu(menu, { menu = false }) {
                        if (i > 0) {
                            KoanMenuItem("Move up", KoanIcons.moveUp) {
                                menu = false
                                dsp.setLayers(d.name, layers.toMutableList().apply { add(i - 1, removeAt(i)) })
                            }
                        }
                        if (i < layers.lastIndex) {
                            KoanMenuItem("Move down", KoanIcons.moveDown) {
                                menu = false
                                dsp.setLayers(d.name, layers.toMutableList().apply { add(i + 1, removeAt(i)) })
                            }
                        }
                        KoanMenuItem("Remove", KoanIcons.remove) {
                            menu = false
                            dsp.setLayers(d.name, layers.filterIndexed { j, _ -> j != i })
                        }
                    }
                }
            }
        }
    }
    item {
        ButtonRow {
            if (addable.isNotEmpty()) AddLayerMenu("Add an EQ", addable) { dsp.setLayers(d.name, layers + DspLayerInfo(it, true)) }
            if (layers.size > 1) KoanButton("Make it a group", { dsp.setGroup(d.name, true) }, kind = KoanButtonKind.Bordered)
        }
    }
    footer(
        "Played in order, before this EQ's own bands. One switched off plays nothing. Removing one takes it out of this list and keeps the EQ. " +
            "A group plays one at a time.",
    )
}

/** The profiles a stack can take, tunings first, since adding one is what most people come for. */
@Composable
private fun AddLayerMenu(title: String, addable: List<DspProfileSummary>, add: (String) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Box {
        KoanButton(title, { open = true }, kind = KoanButtonKind.Bordered)
        KoanMenu(open, { open = false }) {
            (addable.filter { it.role == DspRole.TUNING } + addable.filter { it.role != DspRole.TUNING }).forEach { p ->
                KoanMenuItem(if (p.role == DspRole.TUNING) p.name else "${p.name} · ${p.role.label}") {
                    open = false
                    add(p.name)
                }
            }
        }
    }
}

/** Where a profile is kept: on every device signed in to the account's kōan server, or on this one alone. */
private fun LazyListScope.scopeSection(dsp: DspModel, d: DspProfileDetail) {
    header("Sync")
    item { ChoiceRow("Sync", d.everywhere, listOf("Everywhere" to true, "This device" to false), { dsp.setScope(d.name, it) }) }
    d.syncProblem?.let { line(it, KoanTone.Bad) }
    d.syncNote?.let { line(it, KoanTone.Muted) }
    footer(
        if (d.everywhere) {
            "Everywhere: kept on every device signed in to your kōan server, and an edit on one reaches the rest. Which output plays it " +
                "stays each device's own. Headphone corrections sync by default, since headphones move between devices."
        } else {
            "This device: never leaves it. Room and speaker corrections stay by default, since they belong to where they were measured. " +
                "Moving one here from everywhere removes it from your other devices."
        },
    )
}

/** An impulse response: its rate and channels, its length and peak, and what else it does. */
@Composable
private fun ImpulseRow(ir: DspImpulse) {
    val channels = when (val n = ir.channels?.toInt()) {
        null -> "Every channel"
        1 -> "Mono"
        2 -> "Stereo"
        else -> "$n channels"
    }
    val seconds = ir.taps.toDouble() / ir.rate.toDouble()
    val shape = listOfNotNull(
        "${ir.taps} taps (%.2f s)".format(seconds),
        "peaks at %.1f ms".format(ir.peakMs),
        "mixes channels".takeIf { ir.mixes },
        "delays channels".takeIf { ir.delayed },
    ).joinToString(" · ")
    Column(inset.padding(vertical = KoanSpace.xs)) {
        Row {
            KoanText("${DspModel.khz(ir.rate)} kHz", Modifier.weight(1f))
            KoanText(channels, tone = KoanTone.Muted)
        }
        KoanText(shape, role = KoanType.Fine, tone = KoanTone.Muted)
        KoanText(ir.file, role = KoanType.Fine, tone = KoanTone.Muted)
    }
}

/**
 * A profile's graph, and where it can be edited the graph as an editor: a
 * band's handle dragged for frequency and gain, a graphic curve painted with
 * a brush. The graph follows the finger with a preview the core draws and
 * nothing saved; the edit is saved, and so played, at most every quarter
 * second while it goes on, and once at its end.
 */
@Composable
private fun EqEditor(dsp: DspModel, name: String, detail: DspProfileDetail, response: DspResponse, channel: Int?, onCorrection: Curve? = null) {
    val scope = rememberCoroutineScope()
    var live by remember(response) { mutableStateOf<DspResponse?>(null) }
    var painted by remember(response) { mutableStateOf<List<DspPoint>?>(null) }
    var brush by remember { mutableStateOf(Brush.Narrow) }
    val stepper = remember(name) { EditStepper<DspResponse>(scope, SystemClock::elapsedRealtime) { live = it } }
    DisposableEffect(stepper) { onDispose { stepper.flush() } }
    val editable = !detail.readOnly
    val handles = if (editable) handles(detail.bands, channel ?: 0) else emptyList()
    val curve = if (editable) curveIndex(detail.bands) else null
    val shown = live ?: response
    val accent = KoanTheme.color(KoanTone.Accent)
    val muted = KoanTheme.color(KoanTone.Muted)
    val ink = KoanTheme.color(KoanTone.Ink)
    // The scale holds still while a drag goes on: drawn from what was saved.
    val half = halfRange(listOfNotNull(response.total, response.right, onCorrection?.db))
    // In stereo, the channel being edited, and the other beside it.
    val right = shown.right.takeIf { channel != null }
    val curves = remember(shown.total, right, channel, accent, muted, onCorrection) {
        listOfNotNull(onCorrection) + if (right == null) {
            listOf(Curve(shown.total, accent, 1.5f))
        } else {
            val (edited, other) = if (channel == 1) right to shown.total else shown.total to right
            listOf(Curve(other, muted, 1f), Curve(edited, accent, 1.5f))
        }
    }

    Column(inset.padding(vertical = KoanSpace.s), verticalArrangement = Arrangement.spacedBy(KoanSpace.s)) {
        ResponseGraph(shown.freqs, curves, graphStyle(), Modifier.fillMaxWidth().height(220.dp), fixedHalf = half) {
            Canvas(
                Modifier
                    .fillMaxSize()
                    .pointerInput(handles, curve, brush, half) {
                        if (handles.isEmpty() && curve == null) return@pointerInput
                        val plot = Plot(size.width.toFloat(), size.height.toFloat(), half, LABEL_LEFT.toPx(), LABEL_BOTTOM.toPx())
                        var dragging: Handle? = null
                        var last = Offset.Zero
                        fun move(at: Offset, done: Boolean) {
                            val hz = plot.hz(at.x)
                            val db = plot.db(at.y)
                            val h = dragging
                            if (h != null) {
                                val kind = detail.bands[h.index].kind
                                stepper.step(done, { dsp.previewBand(name, h.index, kind, hz, db, h.q) }) {
                                    dsp.setBand(name, h.index, kind, hz, db, h.q)
                                }
                            } else if (curve != null) {
                                val points = paint(painted ?: detail.bands[curve].curve, hz, db, brush)
                                painted = points
                                stepper.step(done, { dsp.previewCurve(name, curve, points) }) { dsp.setCurve(name, curve, points) }
                            }
                        }
                        detectDragGestures(
                            onDragStart = { at ->
                                last = at
                                val reach = 28.dp.toPx()
                                dragging = handles
                                    .minByOrNull { hypot(plot.x(it.hz) - at.x, plot.y(it.db) - at.y) }
                                    ?.takeIf { hypot(plot.x(it.hz) - at.x, plot.y(it.db) - at.y) < reach }
                            },
                            onDrag = { change, _ ->
                                last = change.position
                                move(last, done = false)
                            },
                            onDragEnd = { move(last, done = true) },
                            onDragCancel = { move(last, done = true) },
                        )
                    },
            ) {
                if (handles.isEmpty()) return@Canvas
                val plot = Plot(size.width, size.height, half, LABEL_LEFT.toPx(), LABEL_BOTTOM.toPx())
                val side = 10.dp.toPx()
                handles.forEach { h ->
                    drawRect(ink, Offset(plot.x(h.hz) - side / 2, plot.y(h.db) - side / 2), Size(side, side), style = Stroke(1.dp.toPx()))
                }
            }
        }
        if (curve != null) {
            KoanSegmented(Brush.entries.map { it.label to it }, brush, { brush = it })
        }
    }
}

/**
 * A profile's filters as a table, its parametric bands editable in place:
 * type, frequency, gain and Q. An edit plays at once. Graphic curves are
 * painted on the graph; delays and mixes are shown as they are.
 */
private fun LazyListScope.bandTable(dsp: DspModel, name: String, d: DspProfileDetail, channel: Int?) {
    if (d.bands.isEmpty() && d.readOnly) return
    header("Filters")
    // In stereo, the bands of the channel being edited, numbered among themselves.
    val shown = d.bands.withIndex().filter { (_, b) -> channel == null || b.on(channel) }
    shown.forEachIndexed { n, (i, band) ->
        item("band-$i-${band.kind}") {
            if (editable(band.kind) && !d.readOnly) {
                BandRow(n + 1, band.kind, band.freq, band.gainDb, band.q, remove = { dsp.removeFilter(name, i) }) { kind, freq, gain, q ->
                    dsp.setBand(name, i, kind, freq, gain, q)
                }
            } else {
                ValueRow("${n + 1}", describe(band.kind, band.freq, band.gainDb, band.q, band.delayMs, band.curve.size))
            }
        }
    }
    if (!d.readOnly) item { ButtonRow { KoanButton("Add a band", { dsp.addBand(name, channel) }, kind = KoanButtonKind.Bordered) } }
    footer(
        if (d.readOnly) {
            "A correction plays as made. To change the sound, add a tuning on top; to edit these, make it a tuning under Role."
        } else {
            "Edits play at once. Frequency, gain and Q are held to 10 Hz–22 kHz, ±30 dB and 0.1–20. Drag a band's handle on " +
                "the graph to move it; drag elsewhere to paint a graphic curve."
        },
    )
}

/** A filter that is not edited in a row, in a line. */
internal fun describe(kind: String, freq: Double, gain: Double, q: Double, delayMs: Double, points: Int) = when (kind) {
    "graphic" -> "Graphic curve, $points points"
    "delay" -> "Delay, %.2f ms".format(delayMs)
    "mix" -> "Mix"
    else -> bandKinds.firstOrNull { it.id == kind }?.let { "${it.name}, %.0f Hz, %.1f dB, Q %.2f".format(freq, gain, q) } ?: kind
}

/** One band's row: its type, and each figure committed when Done is pressed or the field is left. */
@Composable
private fun BandRow(number: Int, kind: String, freq: Double, gain: Double, q: Double, remove: () -> Unit, commit: (String, Double, Double, Double) -> Unit) {
    Row(inset.fillMaxWidth().padding(vertical = KoanSpace.xs), horizontalArrangement = Arrangement.spacedBy(KoanSpace.xs), verticalAlignment = Alignment.CenterVertically) {
        KoanText("$number", Modifier.width(20.dp), role = KoanType.Meta, tone = KoanTone.Muted)
        KoanPicker(bandKinds.map { it.short to it.id }, kind, { commit(it, freq, gain, q) }, keepsCase = true)
        Figure(freq, 0, "Hz", Modifier.weight(1.3f)) { commit(kind, it, gain, q) }
        Figure(gain, 1, "dB", Modifier.weight(1f)) { commit(kind, freq, it, q) }
        Figure(q, 2, "Q", Modifier.weight(1f)) { commit(kind, freq, gain, it) }
        KoanButton(remove, kind = KoanButtonKind.Icon) { KoanIcon(KoanIcons.remove, contentDescription = "Remove band $number") }
    }
}

/** A figure of a band, typed and committed whole: "1" on the way to "1000" is not a frequency anyone set. */
@Composable
private fun Figure(value: Double, digits: Int, unit: String, modifier: Modifier, commit: (Double) -> Unit) {
    val shown = "%.${digits}f".format(value)
    var draft by remember(value) { mutableStateOf<String?>(null) }
    var focused by remember { mutableStateOf(false) }
    val done = {
        draft?.let { typed -> figure(typed, shown, digits) }?.let(commit)
        draft = null
    }
    KoanField(
        draft ?: shown,
        { draft = it },
        modifier.onFocusChanged {
            if (focused && !it.isFocused) done()
            focused = it.isFocused
        },
        placeholder = unit,
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Decimal, imeAction = ImeAction.Done),
        keyboardActions = KeyboardActions(onDone = { done() }),
    )
}

/** What was typed as a figure, if it is one and differs from what is shown. */
internal fun figure(typed: String, shown: String, digits: Int): Double? =
    typed.trim().replace(',', '.').toDoubleOrNull()?.takeIf { "%.${digits}f".format(it) != shown }
