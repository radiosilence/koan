package cc.blit.koan.ui.settings

import android.content.Context
import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.engine.describe
import cc.blit.koan.ffi.DspEarKind
import cc.blit.koan.ffi.DspMeasurementReading
import cc.blit.koan.ffi.DspResponse
import cc.blit.koan.ffi.DspTargetOption
import cc.blit.koan.ffi.KoanException
import cc.blit.koan.ffi.SquigHit
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.navigation.nonce
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanField
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanSegmented
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.Serializable

/** Correcting headphones or a speaker from a measurement, named `name` to begin with. */
@Serializable
data class MeasurementRoute(val name: String = "", val nonce: String = nonce()) : NavKey

/** A step of finding a measurement. */
enum class MeasureStep(val title: String) {
    Learn("Find a measurement"),
    File("Your measurement"),
    Ear("In-ear or over-ear"),
    Target("Choose a target"),
    Review("Check and save"),
}

/** The target most people want for a kind of headphone, as AutoEQ's corrections use: Harman. */
internal fun harman(inEar: Boolean) = if (inEar) "harman-in-ear-2019" else "harman-over-ear-2018"

/**
 * Correcting headphones AutoEQ does not have, from a measurement of them:
 * what a measurement is and where to get one, the file, in-ear or over-ear,
 * a target, and what the correction does before it is saved. Every word it
 * uses is explained once, where it first appears.
 */
@Composable
fun MeasurementPage(initial: String, model: SettingsModel) {
    val dsp = dspModel()
    val nav = LocalNavigator.current
    val context = LocalContext.current
    val uri = LocalUriHandler.current
    val scope = rememberCoroutineScope()
    val settings by model.state.collectAsStateWithLifecycle()
    var step by rememberSaveable { mutableStateOf(MeasureStep.Learn) }
    var name by rememberSaveable { mutableStateOf(initial) }
    var query by rememberSaveable { mutableStateOf(initial) }
    var inEar by rememberSaveable { mutableStateOf(true) }
    var target by rememberSaveable { mutableStateOf<String?>(null) }
    // The measurement itself is held for the page's life alone: too large to save with its state.
    var text by remember { mutableStateOf("") }
    var file by remember { mutableStateOf<String?>(null) }
    var targets by remember { mutableStateOf(emptyList<DspTargetOption>()) }
    var preview by remember { mutableStateOf<DspResponse?>(null) }
    var reading by remember { mutableStateOf<DspMeasurementReading?>(null) }
    var problem by remember { mutableStateOf<String?>(null) }
    var saving by remember { mutableStateOf(false) }
    var hits by remember { mutableStateOf(emptyList<SquigHit>()) }
    var searching by remember { mutableStateOf(false) }
    var searchNote by remember { mutableStateOf<String?>(null) }
    var picking by remember { mutableStateOf<SquigHit?>(null) }
    // The result the measurement came from; null for a file. The check and the credit follow it.
    var fetched by remember { mutableStateOf<SquigHit?>(null) }
    var fetchProblem by remember { mutableStateOf<Pair<SquigHit, String>?>(null) }
    val speaker = reading?.speaker == true

    // Searched once typing pauses; a search overtaken by newer typing is cancelled with its effect.
    LaunchedEffect(query, step) {
        if (step != MeasureStep.File) return@LaunchedEffect
        delay(300)
        val q = query.trim()
        if (q.isEmpty()) {
            hits = emptyList()
            searchNote = null
            return@LaunchedEffect
        }
        searching = true
        try {
            val found = dsp.squigSearch(q)
            hits = found
            searchNote = if (found.isEmpty()) "Nothing on squig.link matches “$q”." else null
        } catch (e: KoanException) {
            hits = emptyList()
            searchNote = e.describe()
        }
        searching = false
    }

    val choose = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        if (uris.isEmpty()) return@rememberLauncherForActivityResult
        scope.launch {
            val read = readTexts(context, uris)
            if (read == null) {
                problem = "That file could not be read as text."
                return@launch
            }
            problem = null
            text = read.second
            file = read.first.joinToString(", ")
            fetched = null
            if (name.isEmpty()) name = read.first.first().substringBeforeLast('.')
        }
    }

    fun canGoOn() = when (step) {
        MeasureStep.Learn, MeasureStep.Ear -> true
        MeasureStep.File -> text.isNotBlank()
        MeasureStep.Target -> target != null
        MeasureStep.Review -> false
    }

    fun next() = scope.launch {
        problem = null
        try {
            when (step) {
                MeasureStep.File -> {
                    // Checked now, so a file that is not a measurement is said so before the questions after it.
                    reading = dsp.describeMeasurement(text)
                    if (reading?.speaker == true) {
                        targets = dsp.targetsFor(DspEarKind.SPEAKER)
                        target = (targets.firstOrNull { it.id == "flat" } ?: targets.firstOrNull())?.id
                        step = MeasureStep.Target
                        return@launch
                    }
                }
                MeasureStep.Ear -> {
                    targets = dsp.targetsFor(if (inEar) DspEarKind.IN_EAR else DspEarKind.OVER_EAR)
                    if (targets.none { it.id == target }) target = (targets.firstOrNull { it.id == harman(inEar) } ?: targets.firstOrNull())?.id
                }
                MeasureStep.Target -> preview = dsp.previewMeasurement(text, target ?: return@launch)
                else -> {}
            }
        } catch (e: KoanException) {
            problem = e.describe()
            return@launch
        }
        step = MeasureStep.entries[step.ordinal + 1]
    }

    fun back() {
        problem = null
        // A speaker has no in-ear or over-ear to ask.
        step = if (step == MeasureStep.Target && speaker) MeasureStep.File else MeasureStep.entries[step.ordinal - 1]
    }

    fun save() {
        val chosen = target ?: return
        saving = true
        scope.launch {
            try {
                val ear = if (speaker) DspEarKind.SPEAKER else if (inEar) DspEarKind.IN_EAR else DspEarKind.OVER_EAR
                dsp.saveMeasured(name, text, ear, chosen, fetched?.source)
                nav.pop()
            } catch (e: KoanException) {
                problem = e.describe()
            }
            saving = false
        }
    }

    SettingsPage(step.title, settings) {
        item {
            ButtonRow {
                KoanButton("Cancel", { nav.pop() }, kind = KoanButtonKind.Text)
                if (step != MeasureStep.Learn) KoanButton("Back", ::back, kind = KoanButtonKind.Text)
                Spacer(Modifier.weight(1f))
                if (step == MeasureStep.Review) {
                    KoanButton("Save", ::save, kind = KoanButtonKind.Prominent, enabled = name.isNotBlank() && !saving)
                } else {
                    KoanButton("Next", { next() }, kind = KoanButtonKind.Prominent, enabled = canGoOn())
                }
            }
        }
        problem?.let { line(it, KoanTone.Bad) }
        when (step) {
            MeasureStep.Learn -> {
                line(
                    "A measurement is a graph of how loud your headphones play each pitch, from deep bass to high treble. " +
                        "People who test headphones publish them.",
                )
                line("You choose a target, the sound you want. kōan works out a correction: what to turn up and down so your headphones sound like the target.")
                header("Where to get one")
                item { ButtonRow { KoanButton("squig.link", { uri.openUri("https://squig.link") }, kind = KoanButtonKind.Link) } }
                line(
                    "Search for your headphones on the next page: kōan looks through the reviewers' squig.link sites. " +
                        "Or save their frequency response as a file from one. REW and most measurement tools export one too.",
                    KoanTone.Muted,
                )
                header("What the file looks like")
                item { KoanText("20, 92.4\n21, 92.6\n22, 92.7\n…", inset.padding(vertical = KoanSpace.s), role = KoanType.Meta, tone = KoanTone.Muted) }
                footer("Two numbers a line: a frequency in hertz, then a level in decibels. Commas, tabs or spaces between them all work.")
                item {
                    ButtonRow {
                        KoanButton("Learn more about headphone EQ", { uri.openUri("https://koan.rocks/docs/headphone-eq/") }, kind = KoanButtonKind.Link)
                    }
                }
            }
            MeasureStep.File -> {
                header("Find it on squig.link")
                item { KoanField(query, { query = it }, inset.fillMaxWidth().padding(vertical = KoanSpace.s), placeholder = "Search…") }
                if (searching && hits.isEmpty()) {
                    line("Searching squig.link…", KoanTone.Muted)
                } else {
                    searchNote?.let { line(it, KoanTone.Muted) }
                }
                hits.take(20).forEach { hit ->
                    item("hit-${hit.site}-${hit.file}") {
                        val details = listOfNotNull(hit.siteLabel, hit.rig?.let { "$it rig" }).joinToString(" · ")
                        if (hit.locked != null) {
                            // Fetching would only fail: the site is offered instead.
                            KoanButton({ uri.openUri(hit.site) }, inset, kind = KoanButtonKind.Card) {
                                Column(Modifier.fillMaxWidth().padding(vertical = KoanSpace.xs)) {
                                    KoanText(hit.name, tone = KoanTone.Muted)
                                    KoanText("$details · opens in a browser", role = KoanType.Fine, tone = KoanTone.Muted)
                                }
                            }
                        } else {
                            KoanButton({
                                picking = hit
                                fetchProblem = null
                                scope.launch {
                                    try {
                                        text = dsp.squigFetch(hit)
                                        fetched = hit
                                        file = "${hit.name}, ${hit.siteLabel}"
                                        problem = null
                                        if (name.isEmpty()) name = "${hit.brand} ${hit.model}"
                                        hit.inEar?.let { inEar = it }
                                    } catch (e: KoanException) {
                                        fetchProblem = hit to e.describe()
                                    }
                                    picking = null
                                }
                            }, inset, kind = KoanButtonKind.Card, enabled = picking == null) {
                                Row(Modifier.fillMaxWidth().padding(vertical = KoanSpace.xs), verticalAlignment = Alignment.CenterVertically) {
                                    Column(Modifier.weight(1f)) {
                                        KoanText(hit.name)
                                        KoanText(details, role = KoanType.Fine, tone = KoanTone.Muted)
                                        fetchProblem?.takeIf { it.first == hit }?.let { KoanText(it.second, role = KoanType.Fine, tone = KoanTone.Bad) }
                                    }
                                    when {
                                        picking == hit -> KoanText("Fetching…", role = KoanType.Fine, tone = KoanTone.Muted, appText = true)
                                        fetched == hit -> KoanIcon(KoanIcons.check, contentDescription = "Chosen", color = KoanTheme.color(KoanTone.Accent))
                                    }
                                }
                            }
                        }
                    }
                }
                hits.take(20).firstNotNullOfOrNull { it.locked }?.let { line("$it.", KoanTone.Muted) }
                footer(
                    "Measurements reviewers publish on their squig.link sites. Pick one measured on the rig your target assumes, where the site " +
                        "says, and to match a site's own EQ presets, its measurement; the site is credited on the correction.",
                )
                item {
                    ButtonRow {
                        KoanButton(file?.let { "Chosen: $it" } ?: "Choose a file…", { choose.launch(arrayOf("*/*")) }, kind = KoanButtonKind.Bordered)
                    }
                }
                footer(
                    "A .csv or .txt file of frequency and level. For a speaker measured by Audio Science Review, choose both its SPL Horizontal " +
                        "and SPL Vertical files.",
                )
                header("Or paste it here")
                item {
                    KoanField(
                        text,
                        {
                            text = it
                            fetched = null
                        },
                        inset.fillMaxWidth().heightIn(min = 140.dp).padding(vertical = KoanSpace.s),
                        placeholder = "20, 92.4",
                        singleLine = false,
                    )
                }
            }
            MeasureStep.Ear -> {
                header("Your headphones are")
                item { KoanSegmented(listOf("In-ear" to true, "Over-ear" to false), inEar, { inEar = it }, inset) }
                footer(
                    if (inEar) "In-ear: earphones and IEMs that sit in your ear canal."
                    else "Over-ear: headphones with cups that sit around or on your ears.",
                )
            }
            MeasureStep.Target -> {
                if (speaker) {
                    reading?.let {
                        header("Read as")
                        line(it.note, KoanTone.Muted)
                    }
                }
                header("Target")
                targets.forEach { t ->
                    item("target-${t.id}") {
                        KoanButton({ target = t.id }, inset, kind = KoanButtonKind.Card) {
                            Row(Modifier.fillMaxWidth().padding(vertical = KoanSpace.xs), verticalAlignment = Alignment.CenterVertically) {
                                Column(Modifier.weight(1f)) {
                                    KoanText(if (t.does.isEmpty()) t.name else "${t.name}: ${t.does}", tone = if (t.id == target) KoanTone.Accent else KoanTone.Ink)
                                    if (t.character.isNotEmpty()) KoanText(t.character, role = KoanType.Fine, tone = KoanTone.Muted)
                                }
                                if (t.id == target) KoanIcon(KoanIcons.check, contentDescription = "Chosen", color = KoanTheme.color(KoanTone.Accent))
                            }
                        }
                    }
                }
                footer("Each target is a different idea of a good sound. You can change it later on the correction's page.")
            }
            MeasureStep.Review -> {
                preview?.let { r ->
                    item { MeasuredGraph(r) }
                    footer(
                        "Measured shows your measurement, the target, and what your ${if (speaker) "speaker" else "headphones"} will sound like " +
                            "with the correction. EQ shows the correction itself.",
                    )
                }
                item { KoanField(name, { name = it }, inset.fillMaxWidth().padding(vertical = KoanSpace.s), placeholder = "Name") }
                footer(
                    "Usually the ${if (speaker) "speaker's" else "headphones'"} name. It becomes a correction: add tunings, like more bass or a " +
                        "room tilt, on top of it.",
                )
            }
        }
    }
}

/** The chosen files' names, and their contents read as one text: a speaker's two planes are read together. `null` where one is not text. */
internal suspend fun readTexts(context: Context, uris: List<Uri>): Pair<List<String>, String>? = withContext(Dispatchers.IO) {
    val read = uris.mapIndexed { i, uri ->
        val text = runCatching { context.contentResolver.openInputStream(uri)?.use { it.readBytes().decodeToString(throwOnInvalidSequence = true) } }
            .getOrNull() ?: return@withContext null
        (displayName(context, uri) ?: "file-$i") to text
    }
    read.map { it.first } to read.joinToString("\n") { it.second }
}

/**
 * A measurement's correction drawn two ways: the measurement, its target
 * and the two together as corrected; or the correction itself.
 */
@Composable
internal fun MeasuredGraph(r: DspResponse) {
    var measured by rememberSaveable { mutableStateOf(true) }
    val accent = KoanTheme.color(KoanTone.Accent)
    val ink = KoanTheme.color(KoanTone.Ink)
    val muted = KoanTheme.color(KoanTone.Muted)
    val keys = if (measured && r.measurement != null) {
        listOfNotNull(
            r.measurement?.let { "Measured" to Curve(it, muted, 1.2f) },
            r.target?.let { "Target" to Curve(it, ink.copy(alpha = 0.55f), 1.2f, dashed = true, pattern = listOf(3f, 2f)) },
            r.predicted?.let { "Corrected" to Curve(it, accent, 2f) },
        )
    } else {
        listOf("EQ" to Curve(r.total, accent, 2f))
    }
    Column(inset.padding(vertical = KoanSpace.s), verticalArrangement = Arrangement.spacedBy(KoanSpace.s)) {
        if (r.measurement != null) KoanSegmented(listOf("Measured" to true, "EQ" to false), measured, { measured = it })
        ResponseGraph(r.freqs, keys.map { it.second }, graphStyle(), Modifier.fillMaxWidth().height(220.dp))
        Legend(keys)
    }
}

/** What each curve on a graph is, as a short stroke of it and its name. */
@Composable
internal fun Legend(keys: List<Pair<String, Curve>>) {
    Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.m), verticalAlignment = Alignment.CenterVertically) {
        keys.forEach { (name, curve) ->
            Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.xs), verticalAlignment = Alignment.CenterVertically) {
                Canvas(Modifier.size(14.dp, 2.dp)) {
                    val effect = if (curve.dashed) PathEffect.dashPathEffect(curve.pattern.map { it.dp.toPx() }.toFloatArray()) else null
                    drawLine(curve.color, Offset(0f, size.height / 2), Offset(size.width, size.height / 2), 2.dp.toPx(), pathEffect = effect)
                }
                KoanText(name, role = KoanType.Fine, tone = KoanTone.Muted, appText = true)
            }
        }
    }
}
