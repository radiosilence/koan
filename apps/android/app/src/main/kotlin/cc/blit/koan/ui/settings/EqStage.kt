package cc.blit.koan.ui.settings

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.ffi.DspJoin
import cc.blit.koan.ffi.DspOverview
import cc.blit.koan.ffi.DspProfileSummary
import cc.blit.koan.ffi.DspRole
import cc.blit.koan.ffi.DspTargets
import cc.blit.koan.ffi.DspTuningEntry
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.navigation.nonce
import cc.blit.koan.ui.theme.KoanBadge
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanFormRow
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanPicker
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import kotlinx.coroutines.launch
import kotlinx.serialization.Serializable

/** What fills a stage of `device`'s chain, to choose. */
@Serializable
data class EqStageRoute(val device: String, val stage: Stage, val nonce: String = nonce()) : NavKey

/** What the words on the EQ page mean. */
@Serializable
data class EqExplainerRoute(val nonce: String = nonce()) : NavKey

/**
 * What fills a stage, chosen: a correction, with the target it aims at and a
 * group's member, or an EQ to add to the filters. Add… at the foot brings in
 * another; an import started here goes into this stage once its role is
 * answered.
 */
@Composable
fun EqStagePage(device: String, stage: Stage, model: SettingsModel) {
    val app = koanApp()
    val dsp = dspModel()
    val nav = LocalNavigator.current
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val settings by model.state.collectAsStateWithLifecycle()
    val state by dsp.state.collectAsStateWithLifecycle()
    val library by app.mirror.library.collectAsStateWithLifecycle()
    var creating by remember { mutableStateOf(false) }
    val shown by produceState<DspOverview?>(null, device, state.version, library) { value = dsp.overviewFor(device) }
    val active = shown?.active
    val targets by produceState<DspTargets?>(null, active, state.version) { value = active?.let { dsp.targets(it) } }
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        if (uris.isNotEmpty()) {
            scope.launch {
                dsp.importPaths(copyIn(context, uris), into = DspPlacement(device, stage))
                nav.pop()
            }
        }
    }
    val chosen = { work: () -> Unit ->
        work()
        nav.pop()
    }
    // In place of this page: what it leads to returns to the EQ page.
    val instead = { route: NavKey ->
        nav.pop()
        nav.push(route)
    }

    SettingsPage(if (stage == Stage.Correction) "Correction" else "Add EQ", settings) {
        val o = shown ?: return@SettingsPage
        val correction = o.profiles.firstOrNull { it.name == o.active }
        when (stage) {
            Stage.Correction -> {
                header("Corrections")
                item { ChoiceCard("None", "No correction: the device as it is", chosen = o.active == null) { chosen { dsp.assign(null, device) } } }
                o.profiles.filter { !it.preset && it.role != DspRole.TUNING }.forEach { p ->
                    item("correction-${p.name}") {
                        ChoiceCard(p.name, DspModel.describe(p), chosen = o.active == p.name) { chosen { dsp.assign(p.name, device) } }
                    }
                }
                o.active?.let { name -> current(name, correction, targets, dsp, split = { instead(SplitRoute(name)) }) }
            }
            Stage.Eq -> {
                val groups = eqGroups(o)
                if (groups.isEmpty()) {
                    header("EQs")
                    line("Every EQ is in the filters already. Make a new one or add another below.", KoanTone.Muted)
                }
                groups.forEach { (title, eqs) ->
                    header(title)
                    eqs.forEach { p ->
                        item("eq-${p.name}") {
                            ChoiceCard(
                                p.name,
                                DspModel.describe(p),
                                madeFor = p.madeFor.takeUnless { o.filtersOnly },
                                matched = p.join == DspJoin.Matched,
                                chosen = false,
                            ) { chosen { dsp.setTunings(o.chain + DspTuningEntry(p.name, true), device) } }
                        }
                    }
                }
            }
        }
        header("Add…")
        if (stage == Stage.Eq) item { ButtonRow { KoanButton("New EQ…", { creating = true }, kind = KoanButtonKind.Bordered) } }
        item { ButtonRow { KoanButton("Import a file…", { pick.launch(arrayOf("*/*")) }, kind = KoanButtonKind.Bordered) } }
        if (!o.filtersOnly) {
            item { ButtonRow { KoanButton("Find in AutoEQ…", { instead(AutoEqRoute()) }, kind = KoanButtonKind.Bordered) } }
            item { ButtonRow { KoanButton("Find a measurement…", { instead(MeasurementRoute()) }, kind = KoanButtonKind.Bordered) } }
        }
        state.error?.let { line(it, KoanTone.Bad) }
    }

    if (creating) {
        NewEqDialog(dsp, device, { creating = false }) { name -> instead(DspProfileRoute(name, device)) }
    }
}

/**
 * The EQs that could be added, made against the target the correction aims
 * at ahead of the rest, each alphabetical; one list where there is no
 * correction to match.
 */
internal fun eqGroups(o: DspOverview): List<Pair<String, List<DspProfileSummary>>> {
    val choices = o.profiles.filter { p -> !p.preset && p.role == DspRole.TUNING && p.rates.isEmpty() && o.chain.none { it.name == p.name } }
    if (choices.isEmpty()) return emptyList()
    if (o.aim == null || o.filtersOnly) return listOf("EQs" to choices)
    val sorted = choices.sortedWith(compareBy(String.CASE_INSENSITIVE_ORDER) { it.name })
    val (matching, other) = sorted.partition { it.join == DspJoin.Matched }
    if (matching.isEmpty()) return listOf("EQs" to other)
    return listOf("Matches your correction" to matching) + (if (other.isEmpty()) emptyList() else listOf("Other" to other))
}

/** The correction chosen: the target it aims at, a group's member, or for one with a tuning baked in, the way to take it apart. */
private fun LazyListScope.current(
    name: String,
    correction: DspProfileSummary?,
    targets: DspTargets?,
    dsp: DspModel,
    split: () -> Unit,
) {
    header(name)
    if (targets != null && correction?.role == DspRole.CORRECTION) {
        item {
            TargetPickerRow(
                "Target",
                targets.chosen ?: targets.madeFor?.id ?: "",
                listOf(null to targets.choices.map { it.id }),
                name = { id ->
                    val t = targets.choices.firstOrNull { it.id == id }
                    if (t != null && id == targets.madeFor?.id) "${t.name} (default)" else t?.name.orEmpty()
                },
                onSelect = { id -> dsp.chooseTarget(name, id.takeUnless { it == targets.madeFor?.id }) },
                describe = { id -> targets.choices.firstOrNull { it.id == id }?.does.orEmpty() },
            )
        }
    }
    if (correction != null && correction.members.isNotEmpty()) {
        item {
            KoanFormRow("Playing", inset) {
                KoanPicker(correction.members.map { it to it }, correction.playing ?: correction.members.first(), { dsp.select(name, it) }, keepsCase = true)
            }
        }
    }
    if (correction?.role == DspRole.BAKED) {
        line("$name already includes a tuning, so no other tuning plays on it. Split it into a correction and a tuning to change that.", KoanTone.Muted)
        if (correction.rates.isEmpty() && correction.layers == 0u) {
            item { ButtonRow { KoanButton("Split into correction + tuning…", split, kind = KoanButtonKind.Bordered) } }
        }
    }
}

/** A choice: its name, for an EQ the target it was made against, in the accent where that is the correction's, and what it holds. */
@Composable
private fun ChoiceCard(name: String, detail: String, madeFor: String? = null, matched: Boolean = false, chosen: Boolean, choose: () -> Unit) {
    KoanButton(choose, inset, kind = KoanButtonKind.Card) {
        Row(Modifier.fillMaxWidth().padding(vertical = KoanSpace.s), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                KoanText(name)
                madeFor?.let { KoanBadge(it, accent = matched) }
                if (detail.isNotEmpty()) KoanText(detail, role = KoanType.Fine, tone = KoanTone.Muted)
            }
            if (chosen) KoanIcon(KoanIcons.check, contentDescription = "Chosen", color = KoanTheme.color(KoanTone.Accent))
        }
    }
}

private val words = listOf(
    "Correction" to "Makes your headphones neutral: measured and brought to a target. A device has one, and it plays as it was made, " +
        "so it is not edited here; filters go on top. Switch Correct for a headphone profile off to play filters alone, as speakers usually do.",
    "Filters" to "Your EQs on top of the correction, or on their own: one or more, played in order, each switched on or off.",
    "EQ" to "One set of bands. Open it from the chain to edit it, or make a new one from Add EQ.",
    "Preset" to "A device's correction and filters saved together, to switch it between, or set another device from.",
    "Flat" to "Nothing chosen: the music plays untouched, bit for bit.",
)

/** What the words on the EQ page mean. */
@Composable
fun EqExplainerPage(model: SettingsModel) {
    val settings by model.state.collectAsStateWithLifecycle()
    SettingsPage("How EQ works", settings) {
        line("Music plays through the correction, then the filters' EQs in order, then out to the device. Each device has its own.")
        header("Words")
        words.forEach { (word, meaning) ->
            item("word-$word") {
                Column(inset.padding(vertical = KoanSpace.s), verticalArrangement = Arrangement.spacedBy(KoanSpace.xs)) {
                    KoanText(word, tone = KoanTone.Strong, appText = true)
                    KoanText(meaning, role = KoanType.Meta, tone = KoanTone.Muted)
                }
            }
        }
        footer(
            "An EQ made against one target and played on a correction aiming at another gets the difference between the two added, " +
                "so it sounds as it was made.",
        )
    }
}
