package cc.blit.koan.ui.settings

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalClipboard
import androidx.compose.ui.platform.LocalContext
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
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanPicker
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanToggle
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.serialization.Serializable

/** AutoEQ's search, open with `query` typed: empty from its button, a model from an offer. */
@Serializable
data class AutoEqRoute(val query: String = "", val nonce: String = nonce()) : NavKey

/** One maker's results in AutoEQ, by model. */
@Serializable
data class AutoEqMakerRoute(val maker: String, val nonce: String = nonce()) : NavKey

/**
 * EQ: what this phone's output plays through, the profiles there are, and
 * getting more. Nothing is chosen at first, and then the music plays
 * untouched by EQ.
 */
@Composable
fun EqPane(model: SettingsModel) {
    val app = koanApp()
    val dsp = dspModel()
    val context = LocalContext.current
    val nav = LocalNavigator.current
    val clipboard = LocalClipboard.current
    val scope = rememberCoroutineScope()
    val settings by model.state.collectAsStateWithLifecycle()
    val state by dsp.state.collectAsStateWithLifecycle()
    val library by app.mirror.library.collectAsStateWithLifecycle()
    var naming by remember { mutableStateOf(false) }
    var adding by remember { mutableStateOf(false) }
    var creating by rememberSaveable { mutableStateOf(false) }
    // Profiles sync from the account's other devices, which the engine
    // reports as the library moving.
    LaunchedEffect(library) { dsp.reload() }
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        if (uris.isNotEmpty()) scope.launch { dsp.importPaths(copyIn(context, uris)) }
    }
    val o = state.overview

    SettingsPage(Pane.Eq.title, settings) {
        state.suggestion?.let { offer ->
            item { Suggestion(offer, dsp) { query -> nav.push(AutoEqRoute(query)) } }
        }
        if (o != null) {
            header("This phone")
            o.device?.let { device -> item { ValueRow("Output", o.names[device] ?: device) } }
            item { OutputGraph(o, state.output) }
            item {
                ChoiceRow("Correction", o.active, correctionOptions(o), dsp::use)
            }
            footer(sentence(o))

            chain(o, dsp, open = { nav.push(DspProfileRoute(it)) }, add = { adding = true })
            presets(o, dsp, save = { naming = true })

            header("Profiles")
            if (o.profiles.isEmpty()) line("None yet. Find one in AutoEQ, or import one below.", KoanTone.Muted)
            o.profiles.forEach { p ->
                item("profile-${p.name}") {
                    KoanButton({ nav.push(DspProfileRoute(p.name)) }, inset, kind = KoanButtonKind.Card) {
                        Column(Modifier.fillMaxWidth().padding(vertical = KoanSpace.xs)) {
                            Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.s), verticalAlignment = Alignment.CenterVertically) {
                                val playing = p.name == o.active || o.chain.any { it.name == p.name && it.on }
                                KoanText(p.name, Modifier.weight(1f), tone = if (playing) KoanTone.Accent else KoanTone.Ink)
                                KoanText(p.role.label, role = KoanType.Fine, tone = KoanTone.Muted, appText = true)
                            }
                            val about = listOfNotNull(DspModel.describe(p).takeIf { it.isNotEmpty() }, p.problem).joinToString(" · ")
                            if (about.isNotEmpty()) KoanText(about, role = KoanType.Fine, tone = if (p.problem != null) KoanTone.Bad else KoanTone.Muted)
                        }
                    }
                }
            }
        }

        header("Add")
        item {
            ButtonRow {
                KoanButton("Find in AutoEQ…", { nav.push(AutoEqRoute()) }, kind = KoanButtonKind.Bordered)
                KoanButton("Import files…", { pick.launch(arrayOf("*/*")) }, kind = KoanButtonKind.Bordered)
            }
        }
        item { ButtonRow { KoanButton("New EQ…", { creating = true }, kind = KoanButtonKind.Bordered) } }
        item {
            ButtonRow {
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
            "AutoEQ's corrections, parametric EQ as text (Equalizer APO, AutoEQ, Wavelet), convolution impulses as WAV, " +
                "or a folder of them zipped. Files shared to kōan, or opened with it, are imported the same way.",
        )
    }

    if (naming) {
        NameDialog("Save as preset", o?.preset ?: "", "Save", { naming = false }) { name -> dsp.savePreset(name) }
    }
    if (adding && o != null) {
        val choices = o.profiles.filter { p -> p.role == DspRole.TUNING && o.chain.none { it.name == p.name } }
        AlertDialog(
            onDismissRequest = { adding = false },
            title = { KoanText("Add a tuning", role = KoanType.TitleSmall, appText = true) },
            text = {
                Column(Modifier.verticalScroll(rememberScrollState())) {
                    if (choices.isEmpty()) KoanText("Every tuning is in already. Import one, or say an EQ is a tuning on its page.", tone = KoanTone.Muted)
                    choices.forEach { p ->
                        KoanButton({
                            adding = false
                            dsp.setTunings(o.chain.added(p.name))
                        }, Modifier.fillMaxWidth(), kind = KoanButtonKind.Card) { KoanText(p.name, Modifier.padding(vertical = KoanSpace.s)) }
                    }
                }
            },
            confirmButton = { KoanButton("Cancel", { adding = false }) },
            dismissButton = {
                KoanButton("New EQ…", {
                    adding = false
                    creating = true
                })
            },
        )
    }
    if (creating) {
        NewEqDialog(dsp, { creating = false }) { name -> nav.push(DspProfileRoute(name)) }
    }
}

/** Where a new EQ can start: flat, or tilted the way most people first reach for. */
private val starters = listOf(
    "Flat" to DspStarter.FLAT,
    "Bass boost" to DspStarter.BASS_BOOST,
    "Warm" to DspStarter.WARM,
    "Bright" to DspStarter.BRIGHT,
)

/**
 * A new EQ: eight bands across the range, flat or from a starting curve,
 * named before it is made. Made, it goes on the end of the tuning and opens
 * to edit.
 */
@Composable
private fun NewEqDialog(dsp: DspModel, dismiss: () -> Unit, made: (String) -> Unit) {
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
                dsp.createEq(chosen, starter) { why ->
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

/**
 * The EQs the output plays on top of its correction, in order: each switched
 * on or off where it stands, moved, or taken out.
 */
private fun LazyListScope.chain(o: DspOverview, dsp: DspModel, open: (String) -> Unit, add: () -> Unit) {
    header("Tuning")
    if (o.chain.isEmpty()) line("None: the correction plays alone.", KoanTone.Muted)
    o.chain.forEachIndexed { i, entry ->
        item("chain-${entry.name}") {
            Row(inset.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                KoanToggle(entry.name, entry.on, { dsp.setTunings(o.chain.toggled(i)) }, Modifier.weight(1f))
                KoanButton("Move up", { dsp.setTunings(o.chain.moved(i, -1)) }, kind = KoanButtonKind.Icon, icon = KoanIcons.moveUp, enabled = i > 0)
                KoanButton("Move down", { dsp.setTunings(o.chain.moved(i, 1)) }, kind = KoanButtonKind.Icon, icon = KoanIcons.moveDown, enabled = i < o.chain.lastIndex)
                KoanButton("Show", { open(entry.name) }, kind = KoanButtonKind.Icon, icon = KoanIcons.disclosure)
                KoanButton("Remove", { dsp.setTunings(o.chain.removed(i)) }, kind = KoanButtonKind.Icon, icon = KoanIcons.remove)
            }
        }
    }
    o.leftOut?.let { line(it, KoanTone.Muted) }
    item { ButtonRow { KoanButton("Add a tuning", add, kind = KoanButtonKind.Bordered) } }
    footer("Taste on top of the correction, played in order. Switched off, an EQ stays where it is and plays nothing.")
}

/**
 * What the Correction picker offers: Off, and the profiles that correct. The
 * one correcting now is always among them, whatever its role says or
 * whether it is listed at all, so the picker never shows a blank.
 */
internal fun correctionOptions(o: DspOverview): List<Pair<String, String?>> {
    val corrections = o.profiles.filter { it.role != DspRole.TUNING }.map { it.name }
    val active = o.active?.takeIf { it !in corrections }
    return listOf<Pair<String, String?>>("Off" to null) + (corrections + listOfNotNull(active)).map { it to it }
}

/** What the output was set from: a preset's name, Flat, or Unsaved for EQ no preset holds. */
internal fun presetSummary(o: DspOverview) = when {
    o.preset != null && o.presetEdited -> "${o.preset}, edited"
    o.preset != null -> o.preset!!
    o.active == null && o.chain.isEmpty() -> "Flat"
    else -> "Unsaved"
}

/** The output's presets: set it from one, or flat; save what it plays as one. */
private fun LazyListScope.presets(o: DspOverview, dsp: DspModel, save: () -> Unit) {
    val presets = o.profiles.filter { it.preset }.map { it.name }
    header("Presets")
    item { ValueRow("Set from", presetSummary(o)) }
    if (presets.isNotEmpty()) {
        // Choosing the one it was set from, edited since, goes back to it.
        item { ChoiceRow("Set to", o.preset, listOf<Pair<String, String?>>("Flat" to null) + presets.map { it to it }, dsp::applyPreset) }
    }
    item { ButtonRow { KoanButton("Save as preset…", save, kind = KoanButtonKind.Bordered) } }
    footer("A preset holds a correction and a tuning, to switch this phone back to, or another device to.")
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
 * What the output plays, drawn: the whole of it in the strongest ink, the
 * correction in the accent, and each EQ of the tuning dashed. A flat output
 * is a line at 0 dB, and says so.
 */
@Composable
private fun OutputGraph(o: DspOverview, response: DspResponse?) {
    val strong = KoanTheme.color(KoanTone.Strong)
    val accent = KoanTheme.color(KoanTone.Accent)
    val muted = KoanTheme.color(KoanTone.Muted)
    val flat = o.active == null && o.chain.none { it.on } || response == null
    val curves = remember(response, o.active, o.chain, flat, strong, accent, muted) {
        response?.takeUnless { flat }?.let { outputCurves(o, it, strong, accent, muted) }.orEmpty()
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

/** The curves the output's graph draws: each stage playing, then the whole. */
internal fun outputCurves(o: DspOverview, r: DspResponse, strong: Color, accent: Color, muted: Color): List<Curve> {
    val eqs = o.chain.filter { it.on }.mapNotNull { entry -> r.layers.firstOrNull { it.name == entry.name } }
    val correction = o.active?.let { r.correction ?: r.total.takeIf { eqs.isEmpty() } }
    return listOfNotNull(correction?.let { Curve(it, accent, 1.5f) }) +
        eqs.map { Curve(it.db, muted, 1f, dashed = true) } +
        Curve(r.total, strong, 2f)
}

/** What the output plays, in a sentence. */
internal fun sentence(o: DspOverview): String {
    val device = o.device?.let { o.names[it] ?: it } ?: "This phone"
    // A correction with a tuning baked in plays alone.
    val baked = o.profiles.firstOrNull { it.name == o.active }?.role == DspRole.BAKED
    val eqs = if (baked) emptyList() else o.chain.filter { it.on && it.name !in o.leftOutEqs }.map { it.name }
    val parts = listOfNotNull(
        o.active?.let { active -> o.aim?.let { "corrected by $active to $it" } ?: "corrected by $active" },
        eqs.takeIf { it.isNotEmpty() }?.let { "through " + listed(it) },
    )
    return if (parts.isEmpty()) "$device is flat: the music plays untouched." else "Music to $device, ${parts.joinToString(", then ")}."
}

/** "a", "a and b", "a, b and c". */
internal fun listed(items: List<String>) = when (items.size) {
    0 -> ""
    1 -> items[0]
    else -> items.dropLast(1).joinToString(", ") + " and " + items.last()
}

@Composable
internal fun NameDialog(title: String, initial: String, action: String, dismiss: () -> Unit, done: (String) -> Unit) {
    var name by rememberSaveable { mutableStateOf(initial) }
    AlertDialog(
        onDismissRequest = dismiss,
        title = { KoanText(title, role = KoanType.TitleSmall, appText = true) },
        text = { KoanField(name, { name = it }, Modifier.fillMaxWidth(), placeholder = "Name") },
        confirmButton = {
            KoanButton(action, {
                done(name)
                dismiss()
            }, enabled = name.isNotBlank())
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
