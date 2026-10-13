package cc.blit.koan.ui.settings

import android.os.SystemClock
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.ffi.DspPoint
import cc.blit.koan.ffi.DspProfileDetail
import cc.blit.koan.ffi.DspResponse
import cc.blit.koan.ffi.DspRole
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.navigation.nonce
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanField
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanPicker
import cc.blit.koan.ui.theme.KoanSegmented
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import kotlin.math.hypot
import kotlinx.serialization.Serializable

/** One EQ or convolution profile's page. */
@Serializable
data class DspProfileRoute(val name: String, val nonce: String = nonce()) : NavKey

/** A profile: what it does, drawn and editable, and what can be done to it. */
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
        val d = detail
        val r = response
        if (r != null && d != null) item { EqEditor(dsp, name, d, r) }
        if (d == null) return@SettingsPage
        d.problem?.let { line(it, KoanTone.Bad) }
        header("Role")
        item { ChoiceRow("This EQ is a", d.role, DspRole.entries.map { it.label to it }, { dsp.setRole(name, it) }) }
        footer(d.role.example)

        if (o != null) {
            header("On this phone")
            val inChain = o.chain.any { it.name == name }
            item {
                ButtonRow {
                    when {
                        name == o.active -> KoanButton("Stop correcting with it", { dsp.use(null) }, kind = KoanButtonKind.Bordered)
                        inChain -> KoanButton("Take out of the tuning", { dsp.setTunings(o.chain.filter { it.name != name }) }, kind = KoanButtonKind.Bordered)
                        d.role == DspRole.TUNING -> KoanButton("Add to the tuning", { dsp.setTunings(o.chain.added(name)) }, kind = KoanButtonKind.Bordered)
                        else -> KoanButton("Correct with it", { dsp.use(name) }, kind = KoanButtonKind.Bordered)
                    }
                }
            }
        }

        summary?.members?.takeIf { it.isNotEmpty() }?.let { members ->
            header("Presets")
            item { ChoiceRow("Playing", summary.playing, members.map { it to it }, { m -> m?.let { dsp.select(name, it) } }) }
        }

        bandTable(dsp, name, d)

        if (d.impulses.isNotEmpty()) {
            header("Impulse responses")
            d.impulses.forEach { i -> item("impulse-${i.file}") { ValueRow(i.file, "${DspModel.khz(i.rate)} kHz · ${i.taps} taps") } }
        }
        header("Headroom")
        item { ValueRow("Pre-amp", "%.1f dB".format(d.preampDb)) }
        footer(
            if (d.preampSet) "Set in this EQ."
            else "Derived at ${DspModel.khz(d.preampRate)} kHz from the largest gain the filters apply, so nothing they boost can clip.",
        )
        if (d.source.isNotEmpty()) {
            header("Imported from")
            d.source.forEach { file -> line(file, KoanTone.Muted) }
        }

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

/**
 * A profile's graph, and where it can be edited the graph as an editor: a
 * band's handle dragged for frequency and gain, a graphic curve painted with
 * a brush. The graph follows the finger with a preview the core draws and
 * nothing saved; the edit is saved, and so played, at most every quarter
 * second while it goes on, and once at its end.
 */
@Composable
private fun EqEditor(dsp: DspModel, name: String, detail: DspProfileDetail, response: DspResponse) {
    val scope = rememberCoroutineScope()
    var live by remember(response) { mutableStateOf<DspResponse?>(null) }
    var painted by remember(response) { mutableStateOf<List<DspPoint>?>(null) }
    var brush by remember { mutableStateOf(Brush.Narrow) }
    val stepper = remember(name) { EditStepper<DspResponse>(scope, SystemClock::elapsedRealtime) { live = it } }
    DisposableEffect(stepper) { onDispose { stepper.flush() } }
    val editable = !detail.readOnly
    val handles = if (editable) handles(detail.bands) else emptyList()
    val curve = if (editable) curveIndex(detail.bands) else null
    val shown = live ?: response
    val accent = KoanTheme.color(KoanTone.Accent)
    val rule = KoanTheme.color(KoanTone.Rule)
    val ink = KoanTheme.color(KoanTone.Ink)
    // The scale holds still while a drag goes on: drawn from what was saved.
    val span = span(response.total)

    Column(inset.padding(vertical = KoanSpace.s), verticalArrangement = Arrangement.spacedBy(KoanSpace.s)) {
        Canvas(
            Modifier
                .fillMaxWidth()
                .height(200.dp)
                .pointerInput(handles, curve, brush) {
                    if (handles.isEmpty() && curve == null) return@pointerInput
                    val scale = GraphScale(size.width.toFloat(), size.height.toFloat(), span)
                    var dragging: Handle? = null
                    fun move(at: Offset, done: Boolean) {
                        val hz = scale.hz(at.x)
                        val db = scale.db(at.y)
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
                    var last = Offset.Zero
                    detectDragGestures(
                        onDragStart = { at ->
                            last = at
                            val reach = 28.dp.toPx()
                            dragging = handles
                                .minByOrNull { hypot(scale.x(it.hz) - at.x, scale.y(it.db) - at.y) }
                                ?.takeIf { hypot(scale.x(it.hz) - at.x, scale.y(it.db) - at.y) < reach }
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
            val scale = GraphScale(size.width, size.height, span)
            drawLine(rule, Offset(0f, scale.y(0.0)), Offset(size.width, scale.y(0.0)), 1.dp.toPx())
            val path = Path()
            shown.freqs.zip(shown.total).forEachIndexed { i, (f, db) ->
                if (i == 0) path.moveTo(scale.x(f), scale.y(db)) else path.lineTo(scale.x(f), scale.y(db))
            }
            drawPath(path, accent, style = Stroke(1.5.dp.toPx()))
            val side = 10.dp.toPx()
            handles.forEach { h ->
                drawRect(ink, Offset(scale.x(h.hz) - side / 2, scale.y(h.db) - side / 2), Size(side, side), style = Stroke(1.dp.toPx()))
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
private fun LazyListScope.bandTable(dsp: DspModel, name: String, d: DspProfileDetail) {
    if (d.bands.isEmpty() && d.readOnly) return
    header("Filters")
    d.bands.forEachIndexed { i, band ->
        item("band-$i-${band.kind}") {
            if (editable(band.kind) && !d.readOnly) {
                BandRow(i + 1, band.kind, band.freq, band.gainDb, band.q, remove = { dsp.removeFilter(name, i) }) { kind, freq, gain, q ->
                    dsp.setBand(name, i, kind, freq, gain, q)
                }
            } else {
                ValueRow("${i + 1}", describe(band.kind, band.freq, band.gainDb, band.q, band.delayMs, band.curve.size))
            }
        }
    }
    if (!d.readOnly) item { ButtonRow { KoanButton("Add a band", { dsp.addBand(name) }, kind = KoanButtonKind.Bordered) } }
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
