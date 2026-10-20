package cc.blit.koan.ui.settings

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.engine.describe
import cc.blit.koan.ffi.DspEarKind
import cc.blit.koan.ffi.DspResponse
import cc.blit.koan.ffi.DspTargetOption
import cc.blit.koan.ffi.KoanException
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.navigation.nonce
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanFormRow
import cc.blit.koan.ui.theme.KoanSegmented
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import kotlinx.coroutines.launch
import kotlinx.serialization.Serializable

/** Taking the baked EQ `name` apart. */
@Serializable
data class SplitRoute(val name: String, val nonce: String = nonce()) : NavKey

/**
 * Taking a baked EQ apart: with a measurement of the headphones and the
 * target that counts as neutral, the correction is target minus
 * measurement, and the tuning is what the EQ does beyond it. Both are drawn
 * against the EQ before anything is saved.
 */
@Composable
fun SplitPage(name: String, model: SettingsModel) {
    val dsp = dspModel()
    val nav = LocalNavigator.current
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val settings by model.state.collectAsStateWithLifecycle()
    var inEar by rememberSaveable { mutableStateOf(true) }
    var target by rememberSaveable { mutableStateOf<String?>(null) }
    var text by remember { mutableStateOf("") }
    var file by remember { mutableStateOf<String?>(null) }
    var targets by remember { mutableStateOf(emptyList<DspTargetOption>()) }
    var preview by remember { mutableStateOf<DspResponse?>(null) }
    var problem by remember { mutableStateOf<String?>(null) }
    var saving by remember { mutableStateOf(false) }

    LaunchedEffect(inEar) {
        targets = dsp.targetsFor(if (inEar) DspEarKind.IN_EAR else DspEarKind.OVER_EAR)
        if (targets.none { it.id == target }) target = (targets.firstOrNull { it.id == harman(inEar) } ?: targets.firstOrNull())?.id
    }
    LaunchedEffect(text, target) {
        val chosen = target ?: return@LaunchedEffect
        if (text.isEmpty()) return@LaunchedEffect
        try {
            preview = dsp.previewSplit(name, text, chosen)
            problem = null
        } catch (e: KoanException) {
            preview = null
            problem = e.describe()
        }
    }
    val choose = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        uri ?: return@rememberLauncherForActivityResult
        scope.launch {
            val read = readTexts(context, listOf(uri))
            if (read == null) {
                problem = "That file could not be read as text."
            } else {
                file = read.first.single()
                text = read.second
            }
        }
    }

    SettingsPage("Split $name", settings) {
        item {
            ButtonRow {
                KoanButton("Cancel", { nav.pop() }, kind = KoanButtonKind.Text)
                Spacer(Modifier.weight(1f))
                KoanButton("Split", {
                    val chosen = target ?: return@KoanButton
                    saving = true
                    scope.launch {
                        try {
                            dsp.splitBaked(name, text, inEar, chosen)
                            nav.pop()
                        } catch (e: KoanException) {
                            problem = e.describe()
                        }
                        saving = false
                    }
                }, kind = KoanButtonKind.Prominent, enabled = preview != null && !saving)
            }
        }
        line(
            "$name is a correction for a device that already includes a tuning. With a measurement of the device, kōan takes it apart: " +
                "a correction that makes it neutral, and the tuning, your taste, which then works on any device.",
        )
        header("Measurement")
        item { ButtonRow { KoanButton(file ?: "Choose a measurement file…", { choose.launch(arrayOf("*/*")) }, kind = KoanButtonKind.Bordered) } }
        item { KoanFormRow("Headphones", inset) { KoanSegmented(listOf("In-ear" to true, "Over-ear" to false), inEar, { inEar = it }) } }
        footer("A frequency response of the device, from squig.link or REW.")
        if (targets.isNotEmpty()) {
            item {
                TargetPickerRow(
                    "Neutral is",
                    target.orEmpty(),
                    listOf(null to targets.map { it.id }),
                    name = { id -> targets.firstOrNull { it.id == id }?.name.orEmpty() },
                    onSelect = { target = it },
                    describe = { id -> targets.firstOrNull { it.id == id }?.does.orEmpty() },
                )
            }
            footer("The target $name was made for. Harman is the usual one; the tuning is what it does beyond it.")
        }
        preview?.let { r ->
            item { SplitGraph(r) }
            footer("Correction and tuning together play as $name does, but for the treble a correction holds back, where measurements disagree.")
        }
        problem?.let { line(it, KoanTone.Bad) }
        line(
            "Split makes “$name correction” and “$name tuning”. The outputs that play $name will play the correction with the tuning on top, " +
                "and $name itself is kept.",
            KoanTone.Muted,
        )
    }
}

/** A split's preview: the correction and the tuning, their sum, and the baked EQ they come from. */
@Composable
private fun SplitGraph(r: DspResponse) {
    val accent = KoanTheme.color(KoanTone.Accent)
    val keys = listOfNotNull(
        r.correction?.let { "Correction" to Curve(it, accent.copy(alpha = 0.7f), 1.2f, dashed = true, pattern = listOf(3f, 2f)) },
        r.tuning?.let { "Tuning" to Curve(it, KoanTheme.color(KoanTone.Ink), 1.2f) },
        r.original?.let { "Original" to Curve(it, KoanTheme.color(KoanTone.Muted), 1.2f, dashed = true, pattern = listOf(3f, 2f)) },
        "Total" to Curve(r.total, accent, 2f),
    )
    Column(inset.padding(vertical = KoanSpace.s), verticalArrangement = Arrangement.spacedBy(KoanSpace.s)) {
        ResponseGraph(r.freqs, keys.map { it.second }, graphStyle(), Modifier.fillMaxWidth().height(220.dp))
        Legend(keys)
    }
}
