package cc.blit.koan.ui.settings

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.LocalClipboard
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.ffi.DspOverview
import cc.blit.koan.ffi.DspProfileDetail
import cc.blit.koan.ffi.DspResponse
import cc.blit.koan.ffi.DspRole
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.navigation.nonce
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanField
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import kotlin.math.log10
import kotlinx.coroutines.launch
import kotlinx.serialization.Serializable

/** One EQ or convolution profile's page. */
@Serializable
data class DspProfileRoute(val name: String, val nonce: String = nonce()) : NavKey

/**
 * EQ: what this phone's output plays through, the profiles there are, and
 * importing more. Nothing is chosen at first, and then the music plays
 * untouched: bit-perfect is the default.
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
    // Profiles sync from the account's other devices, which the engine
    // reports as the library moving.
    androidx.compose.runtime.LaunchedEffect(library) { dsp.reload() }
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        if (uris.isNotEmpty()) scope.launch { dsp.importPaths(copyIn(context, uris)) }
    }
    val o = state.overview

    SettingsPage(Pane.Eq.title, settings) {
        if (o != null) {
            header("This phone")
            o.device?.let { device -> item { ValueRow("Output", o.names[device] ?: device) } }
            item {
                ChoiceRow(
                    "Correction",
                    o.active,
                    listOf<Pair<String, String?>>("Off" to null) + o.profiles.filter { it.role != DspRole.TUNING }.map { it.name to it.name },
                    dsp::use,
                )
            }
            item {
                ChoiceRow(
                    "Tuning",
                    o.tuning,
                    listOf<Pair<String, String?>>("None" to null) + o.profiles.filter { it.role == DspRole.TUNING }.map { it.name to it.name },
                    dsp::setTuning,
                )
            }
            footer(sentence(o))

            header("Profiles")
            if (o.profiles.isEmpty()) line("None yet. Import one below.", KoanTone.Muted)
            o.profiles.forEach { p ->
                item("profile-${p.name}") {
                    KoanButton({ nav.push(DspProfileRoute(p.name)) }, inset, kind = KoanButtonKind.Card) {
                        Column(Modifier.fillMaxWidth().padding(vertical = KoanSpace.xs)) {
                            Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.s), verticalAlignment = Alignment.CenterVertically) {
                                KoanText(p.name, Modifier.weight(1f), tone = if (p.name == o.active || p.name == o.tuning) KoanTone.Accent else KoanTone.Ink)
                                KoanText(p.role.label, role = KoanType.Fine, tone = KoanTone.Muted, appText = true)
                            }
                            val about = listOfNotNull(DspModel.describe(p).takeIf { it.isNotEmpty() }, p.problem).joinToString(" · ")
                            if (about.isNotEmpty()) KoanText(about, role = KoanType.Fine, tone = if (p.problem != null) KoanTone.Bad else KoanTone.Muted)
                        }
                    }
                }
            }
        }

        header("Import")
        item {
            ButtonRow {
                KoanButton("Import files…", { pick.launch(arrayOf("*/*")) }, kind = KoanButtonKind.Bordered)
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
            "Parametric EQ as text (Equalizer APO, AutoEQ, Wavelet), convolution impulses as WAV, or a folder of them " +
                "zipped. Files shared to kōan, or opened with it, are imported the same way.",
        )
    }
}

/** What the output plays, in a sentence. */
internal fun sentence(o: DspOverview): String {
    val device = o.device?.let { o.names[it] ?: it } ?: "This phone"
    val parts = listOfNotNull(
        o.active?.let { active -> o.aim?.let { "corrected by $active to $it" } ?: "corrected by $active" },
        o.tuning?.takeIf { o.tuningPlays }?.let { "then through $it" },
    )
    return if (parts.isEmpty()) "$device is flat: the music plays untouched." else "Music to $device, ${parts.joinToString(", ")}."
}

/** A profile: what it does, drawn, and what can be done to it. */
@Composable
fun DspProfilePage(name: String, model: SettingsModel) {
    val dsp = dspModel()
    val nav = LocalNavigator.current
    val settings by model.state.collectAsStateWithLifecycle()
    val state by dsp.state.collectAsStateWithLifecycle()
    var renaming by remember { mutableStateOf(false) }
    var copying by remember { mutableStateOf(false) }
    var deleting by remember { mutableStateOf(false) }
    // Read again whenever a change lands, here or synced.
    val detail by produceState<DspProfileDetail?>(null, name, state.version) { value = dsp.detail(name) }
    val response by produceState<DspResponse?>(null, name, state.version) { value = dsp.response(name) }
    val summary = state.overview?.profiles?.firstOrNull { it.name == name }
    val o = state.overview

    SettingsPage(name, settings) {
        response?.let { r -> item { ResponseGraph(r) } }
        val d = detail ?: return@SettingsPage
        d.problem?.let { line(it, KoanTone.Bad) }
        header("Role")
        item { ChoiceRow("This EQ is a", d.role, DspRole.entries.map { it.label to it }, { dsp.setRole(name, it) }) }
        footer(d.role.example)

        if (o != null) {
            header("On this phone")
            val playing = name == o.active || name == o.tuning
            item {
                ButtonRow {
                    when {
                        playing && d.role == DspRole.TUNING -> KoanButton("Stop tuning with it", { dsp.setTuning(null) }, kind = KoanButtonKind.Bordered)
                        playing -> KoanButton("Stop correcting with it", { dsp.use(null) }, kind = KoanButtonKind.Bordered)
                        d.role == DspRole.TUNING -> KoanButton("Tune with it", { dsp.setTuning(name) }, kind = KoanButtonKind.Bordered)
                        else -> KoanButton("Correct with it", { dsp.use(name) }, kind = KoanButtonKind.Bordered)
                    }
                }
            }
        }

        summary?.members?.takeIf { it.isNotEmpty() }?.let { members ->
            header("Presets")
            item { ChoiceRow("Playing", summary.playing, members.map { it to it }, { m -> m?.let { dsp.select(name, it) } }) }
        }

        header("What it holds")
        val bands = d.bands.size
        if (bands > 0) item { ValueRow("Filters", "$bands") }
        d.impulses.forEach { i ->
            item("impulse-${i.file}") { ValueRow(i.file, "${DspModel.khz(i.rate)} kHz · ${i.taps} taps") }
        }
        if (d.preampSet) item { ValueRow("Pre-amp", "%.1f dB".format(d.preampDb)) }
        d.source.forEach { file -> line(file, KoanTone.Muted) }

        if (!d.readOnly) {
            header("Manage")
            item {
                ButtonRow {
                    KoanButton("Rename", { renaming = true }, kind = KoanButtonKind.Bordered)
                    KoanButton("Save as new", { copying = true }, kind = KoanButtonKind.Bordered)
                    if (d.canRevert) KoanButton("Revert", { dsp.revert(name) }, kind = KoanButtonKind.Bordered)
                }
            }
            item { ButtonRow { KoanButton("Delete", { deleting = true }, kind = KoanButtonKind.Bordered, destructive = true) } }
        }
        state.error?.let { line(it, KoanTone.Bad) }
    }

    if (renaming) {
        NameDialog("Rename", name, "Rename", { renaming = false }) { new ->
            dsp.rename(name, new) { ok ->
                if (ok) {
                    nav.pop()
                    nav.push(DspProfileRoute(new.trim()))
                }
            }
        }
    }
    if (copying) {
        NameDialog("Save as new", "$name copy", "Save", { copying = false }) { new -> dsp.duplicate(name, new) }
    }
    if (deleting) {
        AlertDialog(
            onDismissRequest = { deleting = false },
            title = { KoanText("Delete $name?", role = KoanType.TitleSmall) },
            text = { KoanText("It is taken off every output that plays it.") },
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

@Composable
private fun NameDialog(title: String, initial: String, action: String, dismiss: () -> Unit, done: (String) -> Unit) {
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
 * What a profile does to the sound, 20 Hz to 20 kHz on a log scale, ±12 dB
 * or as far as it goes: the total in the accent over a zero line.
 */
@Composable
private fun ResponseGraph(r: DspResponse) {
    val accent = KoanTheme.color(KoanTone.Accent)
    val rule = KoanTheme.color(KoanTone.Rule)
    val span = maxOf(12.0, r.total.maxOfOrNull { kotlin.math.abs(it) } ?: 0.0)
    Canvas(inset.fillMaxWidth().height(160.dp).padding(vertical = KoanSpace.s)) {
        val lo = log10(20.0)
        val hi = log10(20_000.0)
        fun x(f: Double) = ((log10(f.coerceIn(20.0, 20_000.0)) - lo) / (hi - lo) * size.width).toFloat()
        fun y(db: Double) = (size.height / 2 - db / span * size.height / 2).toFloat()
        drawLine(rule, Offset(0f, y(0.0)), Offset(size.width, y(0.0)), 1.dp.toPx())
        val path = Path()
        r.freqs.zip(r.total).forEachIndexed { i, (f, db) ->
            if (i == 0) path.moveTo(x(f), y(db)) else path.lineTo(x(f), y(db))
        }
        drawPath(path, accent, style = Stroke(1.5.dp.toPx()))
    }
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
