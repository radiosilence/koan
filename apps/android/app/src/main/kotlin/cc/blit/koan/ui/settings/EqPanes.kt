package cc.blit.koan.ui.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.ffi.AutoEqEntry
import cc.blit.koan.ffi.AutoEqOffer
import cc.blit.koan.ffi.DspOverview
import cc.blit.koan.ffi.DspResponse
import cc.blit.koan.ffi.DspRole
import cc.blit.koan.ffi.DspStarter
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.navigation.nonce
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanField
import cc.blit.koan.ui.theme.KoanFormRow
import cc.blit.koan.ui.theme.KoanSectionHeader
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanPicker
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import kotlinx.coroutines.delay
import kotlinx.serialization.Serializable

/** AutoEQ's search, open with `query` typed: empty from its button, a model from an offer. */
@Serializable
data class AutoEqRoute(val query: String = "", val nonce: String = nonce()) : NavKey

/** One maker's results in AutoEQ, by model. */
@Serializable
data class AutoEqMakerRoute(val maker: String, val nonce: String = nonce()) : NavKey

/**
 * EQ for a device, as the chain it plays: music in, its correction, its
 * filters' EQs in order, the device out. The device and its preset head the
 * page, then the curve of the whole chain, then the chain itself, each stage
 * opening where it is chosen or edited. A device set to play filters alone
 * has no correction stage. Every profile, and bringing more in, is Manage EQ.
 */
@Composable
fun EqPane(model: SettingsModel) {
    val app = koanApp()
    val dsp = dspModel()
    val nav = LocalNavigator.current
    val settings by model.state.collectAsStateWithLifecycle()
    val state by dsp.state.collectAsStateWithLifecycle()
    val library by app.mirror.library.collectAsStateWithLifecycle()
    // The device shown: the output in use until another is picked.
    var picked by rememberSaveable { mutableStateOf<String?>(null) }
    var naming by remember { mutableStateOf<String?>(null) }
    // Profiles sync from the account's other devices, which the engine
    // reports as the library moving.
    LaunchedEffect(library) { dsp.reload() }
    // The old curve stays until the new one is drawn, so the page never
    // empties between presets.
    val shown by produceState<EqShown?>(null, picked, state.version, library) {
        val o = dsp.overviewFor(picked)
        val response = o.device?.let { dsp.outputResponse(it) }
        val curves = (listOfNotNull(o.active) + o.chain.map { it.name }).associateWith { dsp.response(it)?.total.orEmpty() }
        value = EqShown(o, response, curves)
    }
    val inUse = state.overview?.device
    val o = shown?.overview
    // Every write goes to the device whose chain is shown: until a newly picked one's has loaded, the last one's.
    val device = o?.device
    val selected = picked ?: device
    val label = { d: String -> o?.names?.get(d) ?: d }

    SettingsPage(Pane.Eq.title, settings) {
        if (o != null && device != null) {
            item {
                Row(inset.fillMaxWidth().padding(top = KoanSpace.l), verticalAlignment = Alignment.CenterVertically) {
                    KoanSectionHeader("Device and preset", Modifier.weight(1f))
                    KoanButton("How EQ works", { nav.push(EqExplainerRoute()) }, kind = KoanButtonKind.Link, icon = KoanIcons.info)
                }
            }
            item {
                KoanFormRow("Device", inset) {
                    KoanPicker(
                        devices(o, inUse, selected).map { d -> (if (d == inUse) "${label(d)} (${KoanTheme.label("In use")})" else label(d)) to d },
                        selected,
                        { picked = it.takeUnless { d -> d == inUse } },
                        keepsCase = true,
                    )
                }
            }
            item {
                KoanFormRow("Preset", inset) {
                    KoanPicker(presetOptions(o), presetTag(o), { tag ->
                        if (tag != UNSAVED && tag != EDITED) dsp.applyPreset(tag.takeUnless { it == FLAT }, device)
                    }, keepsCase = true)
                }
            }
            item { SwitchRow("Correct for a headphone profile", !o.filtersOnly) { on -> dsp.setFiltersOnly(!on, device) } }
            val preset = o.preset
            if (preset != null && o.presetEdited) {
                item { KoanText("${KoanTheme.label("Changed since")} $preset", inset.padding(top = KoanSpace.s), tone = KoanTone.Muted) }
                item {
                    ButtonRow {
                        KoanButton("Save", { dsp.savePreset(preset, device) }, kind = KoanButtonKind.Bordered)
                        KoanButton("Revert", { dsp.applyPreset(preset, device) }, kind = KoanButtonKind.Link)
                        KoanButton("Save as new…", { naming = "Save as new preset" }, kind = KoanButtonKind.Link)
                    }
                }
            } else if (preset == null && !flat(o)) {
                item { ButtonRow { KoanButton("Save as preset…", { naming = "Save as preset" }, kind = KoanButtonKind.Bordered) } }
            }

            item { OutputGraph(o, shown?.response) }
            item {
                EqChain(
                    o,
                    label(device),
                    shown?.curves.orEmpty(),
                    choose = { stage -> nav.push(EqStageRoute(device, stage)) },
                    open = { name -> nav.push(DspProfileRoute(name, device)) },
                    set = { dsp.setTunings(it, device) },
                    madeFor = dsp::setTunedFor,
                    modifier = inset.padding(vertical = KoanSpace.s),
                )
            }
            footer(sentence(o, label(device)))
        }
        if (device == inUse && o?.filtersOnly != true) {
            state.suggestion?.let { offer -> item { Suggestion(offer, dsp) { query -> nav.push(AutoEqRoute(query)) } } }
        }
        state.summary?.let { line(it, KoanTone.Muted) }
        state.error?.let { line(it, KoanTone.Bad) }
        item { ButtonRow { KoanButton("Manage EQ", { nav.push(ManageEqRoute(device)) }, kind = KoanButtonKind.Link) } }
    }

    naming?.let { title ->
        NameDialog(
            title,
            "",
            "Save",
            { naming = null },
            message = "The correction and filters, to switch ${device?.let(label) ?: "a device"} back to, or another device to.",
            // A new name never writes over another preset: only Save changes one.
            taken = { name ->
                "There is already a preset called $name. Choose another name, or change it with Save."
                    .takeIf { o?.profiles?.any { it.preset && it.name == name } == true }
            },
        ) { name -> device?.let { dsp.savePreset(name, it) } }
    }
}

/** What the EQ page draws: the device's overview, its chain's curve, and each stage's own. */
private data class EqShown(val overview: DspOverview, val response: DspResponse?, val curves: Map<String, List<Double>>)

/** Whether a device plays untouched: nothing chosen at all. */
private fun flat(o: DspOverview) = o.active == null && o.chain.isEmpty()

private const val FLAT = "\u0000flat"
private const val UNSAVED = "\u0000unsaved"
private const val EDITED = "\u0000edited"

/** The preset picker's choice: the preset, edited, Flat or Unsaved. */
private fun presetTag(o: DspOverview) = when (val preset = o.preset) {
    null -> if (flat(o)) FLAT else UNSAVED
    else -> if (o.presetEdited) EDITED else preset
}

/** Flat, Unsaved where nothing saved holds what plays, and the presets; one changed since as edited, and as saved, which goes back to it. */
private fun presetOptions(o: DspOverview): List<Pair<String, String>> =
    listOf("Flat" to FLAT) +
        (if (o.preset == null && !flat(o)) listOf("Unsaved" to UNSAVED) else emptyList()) +
        o.profiles.filter { it.preset }.flatMap { p ->
            (if (p.name == o.preset && o.presetEdited) listOf("${p.name} (edited)" to EDITED) else emptyList()) + (p.name to p.name)
        }

/** The outputs to choose among: the one in use first, then the one shown, then any the EQ names. */
internal fun devices(o: DspOverview, inUse: String?, shown: String?): List<String> =
    (listOfNotNull(inUse, shown) + o.profiles.filter { !it.preset }.flatMap { it.devices } + o.tunings.keys.sorted()).distinct()

/** Where a new EQ can start: flat, or tilted the way most people first reach for. */
private val starters = listOf(
    "Flat" to DspStarter.FLAT,
    "Bass boost" to DspStarter.BASS_BOOST,
    "Warm" to DspStarter.WARM,
    "Bright" to DspStarter.BRIGHT,
)

/**
 * A new EQ: eight bands across the range, flat or from a starting curve,
 * named before it is made. Made, it goes on the end of `device`'s filters
 * and opens to edit.
 */
@Composable
internal fun NewEqDialog(dsp: DspModel, device: String?, dismiss: () -> Unit, made: (String) -> Unit) {
    var name by rememberSaveable { mutableStateOf("") }
    var starter by rememberSaveable { mutableStateOf(DspStarter.FLAT) }
    var refusal by remember { mutableStateOf<String?>(null) }
    var saving by remember { mutableStateOf(false) }
    // Held open while the EQ is made: what happens next is this dialog's.
    AlertDialog(
        onDismissRequest = { if (!saving) dismiss() },
        title = { KoanText("New EQ", role = KoanType.TitleSmall, appText = true) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(KoanSpace.s)) {
                KoanField(name, {
                    if (!saving) {
                        name = it
                        refusal = null
                    }
                }, Modifier.fillMaxWidth(), placeholder = "Name")
                KoanFormRow("Start from") { KoanPicker(starters, starter, { starter = it }) }
                KoanText(
                    "A low shelf, six peaks an octave apart from 200 Hz, and a high shelf, each at 0 dB unless the " +
                        "starting curve says otherwise. Every band can be moved, retyped or removed.",
                    role = KoanType.Fine,
                    tone = KoanTone.Muted,
                )
                refusal?.let { KoanText(it, role = KoanType.Fine, tone = KoanTone.Bad) }
            }
        },
        confirmButton = {
            KoanButton("Create", {
                saving = true
                val chosen = name.trim()
                dsp.createEq(chosen, starter, device) { why ->
                    saving = false
                    if (why == null) {
                        dismiss()
                        made(chosen)
                    } else {
                        refusal = why
                    }
                }
            }, enabled = name.isNotBlank() && !saving)
        },
        dismissButton = { KoanButton("Cancel", dismiss, enabled = !saving) },
    )
}

/** What the output was set from: a preset's name, Flat, or Unsaved for EQ no preset holds. */
internal fun presetSummary(o: DspOverview) = when {
    o.preset != null && o.presetEdited -> "${o.preset}, edited"
    o.preset != null -> o.preset!!
    o.active == null && o.chain.isEmpty() -> "Flat"
    else -> "Unsaved"
}

/** AutoEQ's offer for the output in use: its profile, or a search for the model. Nothing applies until asked. */
@Composable
private fun Suggestion(offer: AutoEqOffer, dsp: DspModel, find: (String) -> Unit) {
    Column(inset.padding(vertical = KoanSpace.s), verticalArrangement = Arrangement.spacedBy(KoanSpace.s)) {
        when (offer) {
            is AutoEqOffer.Profile -> {
                KoanText("AutoEQ has a correction for ${offer.entry.name}. Use it?", tone = KoanTone.Muted)
                Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.m), verticalAlignment = Alignment.CenterVertically) {
                    KoanButton("Use", { dsp.installAutoEq(offer.entry) }, kind = KoanButtonKind.Prominent)
                    KoanButton("Not for this device", dsp::dismissSuggestion, kind = KoanButtonKind.Link)
                }
            }
            is AutoEqOffer.Search -> Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.m), verticalAlignment = Alignment.CenterVertically) {
                KoanButton("Find ${offer.query} in AutoEQ…", { find(offer.query) }, kind = KoanButtonKind.Bordered)
                KoanButton("Not for this device", dsp::dismissSuggestion, kind = KoanButtonKind.Link)
            }
        }
    }
}

/**
 * AutoEQ's results by headphone name; with nothing typed, its makers to
 * browse. Choosing one installs it and corrects the output in use with it.
 */
@Composable
fun AutoEqPage(initial: String, model: SettingsModel) {
    val dsp = dspModel()
    val nav = LocalNavigator.current
    val settings by model.state.collectAsStateWithLifecycle()
    val state by dsp.state.collectAsStateWithLifecycle()
    var query by rememberSaveable { mutableStateOf(initial) }
    // Debounced: a search runs once typing pauses, not per keystroke.
    LaunchedEffect(query) {
        if (query.isBlank()) {
            dsp.loadAutoEqMakers()
        } else {
            delay(250)
            dsp.searchAutoEq(query)
        }
    }
    SettingsPage("AutoEQ", settings) {
        item { KoanField(query, { query = it }, inset.fillMaxWidth().padding(vertical = KoanSpace.s), placeholder = "Headphone") }
        if (query.isBlank()) {
            state.autoEqMakers.forEach { maker ->
                item("maker-${maker.name}") {
                    KoanButton({ nav.push(AutoEqMakerRoute(maker.name)) }, inset, kind = KoanButtonKind.Card) {
                        Row(Modifier.fillMaxWidth().padding(vertical = KoanSpace.s)) {
                            KoanText(maker.name, Modifier.weight(1f))
                            KoanText("${maker.results}", tone = KoanTone.Muted)
                        }
                    }
                }
            }
        } else {
            if (state.autoEqResults.isEmpty()) line("AutoEQ has nothing for “${query.trim()}”.", KoanTone.Muted)
            state.autoEqResults.forEach { entry ->
                item("result-${entry.profileName}") {
                    AutoEqRow(entry) {
                        dsp.installAutoEq(entry)
                        nav.pop()
                    }
                }
            }
        }
        state.error?.let { line(it, KoanTone.Bad) }
    }
}

/** A maker's results, by model; where several people measured one, the one AutoEQ recommends comes first. */
@Composable
fun AutoEqMakerPage(maker: String, model: SettingsModel) {
    val dsp = dspModel()
    val nav = LocalNavigator.current
    val settings by model.state.collectAsStateWithLifecycle()
    val models by produceState(emptyList<AutoEqEntry>(), maker) { value = dsp.autoEqModels(maker) }
    SettingsPage(maker, settings) {
        models.forEach { entry ->
            item("model-${entry.profileName}") {
                AutoEqRow(entry) {
                    dsp.installAutoEq(entry)
                    // Back past the search too: the choice is made.
                    nav.pop()
                    nav.pop()
                }
            }
        }
    }
}

/** One AutoEQ result: the headphone, and who measured it. */
@Composable
private fun AutoEqRow(entry: AutoEqEntry, choose: () -> Unit) {
    KoanButton(choose, inset, kind = KoanButtonKind.Card) {
        Column(Modifier.fillMaxWidth().padding(vertical = KoanSpace.xs)) {
            KoanText(entry.name)
            KoanText(entry.measuredBy, role = KoanType.Fine, tone = KoanTone.Muted)
        }
    }
}

/**
 * What a device plays, drawn: the whole of it in the strongest ink, the
 * correction in the accent, each EQ of the tuning as its block draws it,
 * and the right channel where it differs. A flat device is a line at 0 dB,
 * and says so.
 */
@Composable
private fun OutputGraph(o: DspOverview, response: DspResponse?) {
    val strong = KoanTheme.color(KoanTone.Strong)
    val accent = KoanTheme.color(KoanTone.Accent)
    val ink = KoanTheme.color(KoanTone.Ink)
    val muted = KoanTheme.color(KoanTone.Muted)
    val flat = o.active == null && o.chain.none { it.on } || response == null
    val curves = remember(response, o.active, o.chain, flat, strong, accent, ink, muted) {
        response?.takeUnless { flat }?.let { outputCurves(o, it, strong, accent, ink, muted) }.orEmpty()
    }
    val freqs = response?.freqs ?: flatFreqs
    ResponseGraph(freqs, curves, graphStyle(), inset.fillMaxWidth().height(200.dp).padding(vertical = KoanSpace.s)) {
        if (flat) {
            Box(Modifier.fillMaxWidth().padding(top = KoanSpace.l), contentAlignment = Alignment.TopCenter) {
                KoanText("Flat: plays untouched", role = KoanType.Meta, tone = KoanTone.Muted, appText = true)
            }
        }
    }
}

/** A flat output's axis: nothing to draw on it but the frame. */
private val flatFreqs = listOf(20.0, 20_000.0)

/** The curves a device's graph draws: each stage playing, as its block draws it, then the whole. */
internal fun outputCurves(o: DspOverview, r: DspResponse, strong: Color, accent: Color, ink: Color, muted: Color): List<Curve> {
    val eqs = o.chain.withIndex().filter { it.value.on }.mapNotNull { (i, entry) -> r.layers.firstOrNull { it.name == entry.name }?.let { i to it } }
    val correction = o.active?.let { r.correction ?: r.total.takeIf { eqs.isEmpty() } }
    val dash = { s: StageStroke -> s.pattern ?: listOf(6f, 4f) }
    return listOfNotNull(correction?.let { Curve(it, accent, 1.5f) }) +
        eqs.map { (i, layer) -> Curve(layer.db, ink, 1f, dashed = true, pattern = dash(StageStroke.eq(i))) } +
        listOfNotNull(r.right?.let { Curve(it, muted, 1f, dashed = true, pattern = dash(StageStroke.right)) }) +
        Curve(r.total, strong, 2f)
}

@Composable
internal fun NameDialog(
    title: String,
    initial: String,
    action: String,
    dismiss: () -> Unit,
    message: String? = null,
    taken: (String) -> String? = { null },
    done: (String) -> Unit,
) {
    var name by rememberSaveable { mutableStateOf(initial) }
    val refusal = taken(name.trim())
    AlertDialog(
        onDismissRequest = dismiss,
        title = { KoanText(title, role = KoanType.TitleSmall, appText = true) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(KoanSpace.s)) {
                (refusal ?: message)?.let { KoanText(it, role = KoanType.Fine, tone = if (refusal != null) KoanTone.Bad else KoanTone.Muted) }
                KoanField(name, { name = it }, Modifier.fillMaxWidth(), placeholder = "Name")
            }
        },
        confirmButton = {
            KoanButton(action, {
                done(name)
                dismiss()
            }, enabled = name.isNotBlank() && refusal == null)
        },
        dismissButton = { KoanButton("Cancel", dismiss) },
    )
}

/**
 * The questions an import asks wherever it started, Settings or a share: the
 * rate of coefficients that carry none, the name of several files, and what
 * the new profiles are for.
 */
@Composable
fun DspPrompts(dsp: DspModel) {
    val state by dsp.state.collectAsStateWithLifecycle()
    state.needsRate?.let {
        AlertDialog(
            onDismissRequest = dsp::cancelRate,
            title = { KoanText("What sample rate is it at?", role = KoanType.TitleSmall, appText = true) },
            text = {
                Column(Modifier.verticalScroll(rememberScrollState())) {
                    KoanText("These coefficients carry no rate of their own. Use the one the filter was designed at.", tone = KoanTone.Muted)
                    DspModel.rates.forEach { rate -> KoanButton("${DspModel.khz(rate)} kHz", { dsp.retry(rate) }) }
                }
            },
            confirmButton = {},
            dismissButton = { KoanButton("Cancel", dsp::cancelRate) },
        )
    }
    state.pendingImport?.let { pending ->
        var name by remember(pending) { mutableStateOf(pending.plan.name) }
        val plan = pending.plan
        AlertDialog(
            onDismissRequest = dsp::cancelImport,
            title = {
                KoanText(
                    if (plan.group) "These ${plan.files.size} presets will become a group" else "These ${plan.files.size} files will be combined into one EQ",
                    role = KoanType.TitleSmall,
                )
            },
            text = {
                Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(KoanSpace.s)) {
                    plan.files.forEach { KoanText(it, role = KoanType.Meta) }
                    KoanText(
                        if (plan.group) {
                            "Each becomes an EQ of its own, and the group holds them. A group plays one at a time: the first, until you pick another on the group's page."
                        } else {
                            "They are parts of one setup, such as a response for each channel, so they play together as one EQ."
                        },
                        role = KoanType.Fine,
                        tone = KoanTone.Muted,
                    )
                    KoanField(name, { name = it }, Modifier.fillMaxWidth(), placeholder = if (plan.group) "Group name" else "Name")
                }
            },
            confirmButton = { KoanButton("Import", { dsp.confirmImport(name) }, enabled = name.isNotBlank()) },
            dismissButton = { KoanButton("Cancel", dsp::cancelImport) },
        )
    }
    state.askRole?.let { ask ->
        val many = ask.names.size > 1
        AlertDialog(
            // Dismissed, as Decide later.
            onDismissRequest = { dsp.answer(ask, null) },
            title = { KoanText(if (many) "What are these EQs?" else "What is this EQ?", role = KoanType.TitleSmall, appText = true) },
            text = {
                Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(KoanSpace.s)) {
                    KoanText(
                        if (many) "kōan can't tell from the files what these EQs do." else "kōan can't tell from the file what ${ask.names.first()} does.",
                    )
                    ask.into?.let { into ->
                        val device = state.overview?.names?.get(into.device) ?: into.device
                        KoanText(
                            "A tuning is added to $device's filters; a correction becomes its correction.",
                            role = KoanType.Fine,
                            tone = KoanTone.Muted,
                        )
                    }
                    listOf(DspRole.CORRECTION, DspRole.BAKED, DspRole.TUNING).forEach { role ->
                        KoanButton({ dsp.answer(ask, role) }, Modifier.fillMaxWidth(), kind = KoanButtonKind.Card) {
                            Column(Modifier.fillMaxWidth().padding(vertical = KoanSpace.xs)) {
                                KoanText(role.label, appText = true)
                                KoanText(role.example, role = KoanType.Fine, tone = KoanTone.Muted)
                            }
                        }
                    }
                    KoanText(
                        if (many) "Decide later: they're kept as tunings, and you can change each one's role on its page."
                        else "Decide later: it's kept as a tuning, and you can change its role on its page.",
                        role = KoanType.Fine,
                        tone = KoanTone.Muted,
                    )
                }
            },
            confirmButton = { KoanButton("Decide later", { dsp.answer(ask, null) }, kind = KoanButtonKind.Link) },
        )
    }
}
