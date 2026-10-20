package cc.blit.koan.ui.settings

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import cc.blit.koan.ffi.DspEqJoin
import cc.blit.koan.ffi.DspJoin
import cc.blit.koan.ffi.DspOverview
import cc.blit.koan.ffi.DspProfileSummary
import cc.blit.koan.ffi.DspRole
import cc.blit.koan.ffi.DspTuningEntry
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanHairline
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanMenu
import cc.blit.koan.ui.theme.KoanMenuItem
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanToggle
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import kotlin.math.abs

/**
 * How a stage is drawn, the same in its block and on the graph: the
 * correction in the accent, each EQ in ink with a dash of its own, the whole
 * chain in the strongest ink, and its right channel, where it differs,
 * dashed in `muted`.
 */
data class StageStroke(val tone: KoanTone, val pattern: List<Float>? = null) {
    companion object {
        val correction = StageStroke(KoanTone.Accent)
        val total = StageStroke(KoanTone.Strong)
        val right = StageStroke(KoanTone.Muted, listOf(3f, 2f))
        private val dashes = listOf(listOf(5f, 3f), listOf(2f, 2f), listOf(8f, 3f, 2f, 3f), listOf(1f, 4f))

        fun eq(index: Int) = StageStroke(KoanTone.Ink, dashes[index % dashes.size])
    }
}

/** `db` drawn as `stroke` draws it, `width` dp wide. */
@Composable
fun StageStroke.curve(db: List<Double>, width: Float = 1.2f) =
    Curve(db, KoanTheme.color(tone), width, dashed = pattern != null, pattern = pattern ?: listOf(6f, 4f))

/**
 * A curve as a small line, for a stage's block. `range` is the dB at the top
 * edge, shared where thumbnails are compared; the curve's own peak, at least
 * 6 dB, otherwise.
 */
@Composable
fun CurveThumb(db: List<Double>, stroke: StageStroke, range: Double? = null) {
    val color = KoanTheme.color(stroke.tone)
    Canvas(Modifier.size(64.dp, 26.dp)) {
        if (db.size < 2) return@Canvas
        val top = range ?: maxOf(6.0, db.maxOf { abs(it) })
        val path = Path()
        db.forEachIndexed { i, v ->
            val x = size.width * i / (db.size - 1)
            val y = (size.height / 2 - size.height / 2 * (v / top)).toFloat()
            if (i == 0) path.moveTo(x, y) else path.lineTo(x, y)
        }
        val effect = stroke.pattern?.let { p -> PathEffect.dashPathEffect(p.map { it.dp.toPx() }.toFloatArray()) }
        drawPath(path, color, style = Stroke(1.5.dp.toPx(), pathEffect = effect))
    }
}

/**
 * The chain as blocks joined by a line: music in, the correction, the
 * filters' EQs, the device out. Playing filters alone, the correction and
 * everything about targets are left out. An empty stage is a dashed place to
 * add one. Above each EQ, how it meets the correction: the line in the
 * accent where it was made against the correction's target, the conversion
 * kōan plays where it was made against another, and a prompt where that is
 * not set, since then a target may be applied twice. Each EQ is moved and
 * taken out from its menu; taking one out never deletes it.
 */
@Composable
fun EqChain(
    o: DspOverview,
    device: String,
    curves: Map<String, List<Double>>,
    choose: (Stage) -> Unit,
    open: (String) -> Unit,
    set: (List<DspTuningEntry>) -> Unit,
    madeFor: (String, String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val correction = o.profiles.firstOrNull { it.name == o.active }
    val matched = matched(o, correction)
    val accent = KoanTheme.look.accent
    val rule = KoanTheme.color(KoanTone.Rule)
    val line = if (matched) accent else rule
    val width = if (matched) 2.dp else KoanHairline
    val outline = if (matched) accent else null

    Column(modifier) {
        End("Music in", KoanIcons.track, appText = true)
        Link(line, width)
        if (!o.filtersOnly) {
            if (correction != null) {
                StageBlock(
                    title = "Correction",
                    name = correction.name,
                    detail = if (correction.role == DspRole.BAKED) {
                        KoanTheme.label("Already includes a tuning")
                    } else {
                        listOfNotNull(DspModel.describe(correction).takeIf { it.isNotEmpty() }, o.aim?.let { "to $it" }).joinToString(" · ")
                    },
                    db = curves[correction.name],
                    stroke = StageStroke.correction,
                    outline = outline,
                    action = { choose(Stage.Correction) },
                ) {
                    KoanButton("Show", { open(correction.name) }, kind = KoanButtonKind.Link)
                }
            } else {
                Placeholder("Correction", "Add a correction", null) { choose(Stage.Correction) }
            }
            Link(line, width)
        }
        KoanText("Filters", Modifier.padding(vertical = KoanSpace.xs), role = KoanType.Fine, tone = KoanTone.Muted, appText = true)

        o.chain.forEachIndexed { i, entry ->
            val meets = o.joins.getOrNull(i)
            if (o.filtersOnly) {
                if (i > 0) Link(line, width)
            } else if (i > 0 || meets?.join != null || meets?.note != null) {
                Join(meets, entry.name, if (meets?.join == DspJoin.Matched || matched) accent else rule, if (meets?.join == DspJoin.Matched || matched) 2.dp else KoanHairline, open, madeFor)
            }
            var menu by remember { mutableStateOf(false) }
            StageBlock(
                title = "EQ ${i + 1}",
                name = entry.name,
                detail = if (entry.on) meets?.madeFor?.takeUnless { o.filtersOnly }?.let { "made for $it" } else KoanTheme.label("Off"),
                db = curves[entry.name],
                stroke = StageStroke.eq(i),
                outline = outline,
                action = { open(entry.name) },
            ) {
                KoanToggle("On", entry.on, { on -> set(o.chain.mapIndexed { j, e -> if (j == i) DspTuningEntry(e.name, on) else e }) }, Modifier.width(76.dp))
                Box {
                    KoanButton("Options for ${entry.name}", { menu = true }, kind = KoanButtonKind.Icon, icon = KoanIcons.more)
                    KoanMenu(menu, { menu = false }) {
                        KoanMenuItem("Edit", KoanIcons.disclosure) {
                            menu = false
                            open(entry.name)
                        }
                        if (i > 0) {
                            KoanMenuItem("Move up", KoanIcons.moveUp) {
                                menu = false
                                set(o.chain.moved(i, -1))
                            }
                        }
                        if (i < o.chain.lastIndex) {
                            KoanMenuItem("Move down", KoanIcons.moveDown) {
                                menu = false
                                set(o.chain.moved(i, 1))
                            }
                        }
                        KoanMenuItem("Remove", KoanIcons.remove) {
                            menu = false
                            set(o.chain.removed(i))
                        }
                    }
                }
            }
        }

        if (o.chain.isNotEmpty()) Link(line, width)
        if (correction?.role != DspRole.BAKED) {
            Placeholder(null, "Add EQ", outline) { choose(Stage.Eq) }
            Link(line, width)
        }
        End("$device out", KoanIcons.speaker, appText = false)
    }
}

/**
 * The first EQ that plays meets the correction matched, and none after it
 * was made against another target: the whole chain is drawn in the accent.
 * An EQ that says no target (a generic one stacked on top) leaves it
 * matched. What plays is what `sentence` says plays.
 */
internal fun matched(o: DspOverview, correction: DspProfileSummary?): Boolean {
    if (correction == null || correction.role == DspRole.BAKED || !o.tuningPlays) return false
    val playing = o.chain.zip(o.joins).filter { (entry, _) -> entry.on && entry.name !in o.leftOutEqs }
    val first = playing.firstOrNull() ?: return false
    if (first.second.join != DspJoin.Matched) return false
    return playing.drop(1).all { it.second.join == DspJoin.Matched || it.second.join == DspJoin.Unknown }
}

@Composable
private fun Link(color: Color, width: Dp) {
    Box(Modifier.padding(start = 18.dp).width(width).height(14.dp).background(color))
}

@Composable
private fun End(title: String, icon: Int, appText: Boolean) {
    Row(Modifier.padding(start = KoanSpace.s), horizontalArrangement = Arrangement.spacedBy(KoanSpace.s), verticalAlignment = Alignment.CenterVertically) {
        KoanIcon(icon, contentDescription = null, color = KoanTheme.color(KoanTone.Muted))
        KoanText(title, role = KoanType.Meta, tone = KoanTone.Muted, appText = appText)
    }
}

/** The line into an EQ, saying how it meets the correction, and what of it does not play as chosen. */
@Composable
private fun Join(meets: DspEqJoin?, eq: String, color: Color, width: Dp, open: (String) -> Unit, madeFor: (String, String) -> Unit) {
    Column(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 14.dp)
            .drawBehind {
                val x = 18.dp.toPx() + width.toPx() / 2
                drawLine(color, Offset(x, 0f), Offset(x, size.height), width.toPx())
            }
            .padding(start = 18.dp + KoanSpace.m, top = KoanSpace.xs, bottom = KoanSpace.xs),
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        when (val join = meets?.join) {
            DspJoin.Matched -> Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.xs), verticalAlignment = Alignment.CenterVertically) {
                KoanIcon(KoanIcons.check, contentDescription = null, color = KoanTheme.color(KoanTone.Accent))
                KoanText("Matched", role = KoanType.Fine, tone = KoanTone.Accent, appText = true)
            }
            is DspJoin.Converted -> Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.m), verticalAlignment = Alignment.CenterVertically) {
                KoanText("Target difference: ${join.from} → ${join.to}", role = KoanType.Fine, tone = KoanTone.Muted)
                meets.step.takeIf { it.size > 1 }?.let { CurveThumb(it, StageStroke(KoanTone.Muted)) }
            }
            is DspJoin.Refitted -> KoanText("Correction fitted to ${join.to} for it", role = KoanType.Fine, tone = KoanTone.Muted)
            // Not saying is not a mismatch: a generic EQ has no target to say.
            DspJoin.Unknown -> {
                KoanButton("Made against: not set. Set it", { open(eq) }, kind = KoanButtonKind.Link)
                meets.suggestion?.let { s ->
                    KoanButton("Looks made for ${s.name}. Use that?", { madeFor(eq, s.id) }, kind = KoanButtonKind.Link)
                }
            }
            null -> {}
        }
        meets?.note?.let { note ->
            Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.xs), verticalAlignment = Alignment.CenterVertically) {
                KoanIcon(KoanIcons.warning, contentDescription = null, color = KoanTheme.color(KoanTone.Bad))
                KoanText(note, role = KoanType.Fine, tone = KoanTone.Bad)
            }
        }
    }
}

/** A stage of the chain: what it is, what fills it, its curve, and its controls. */
@Composable
private fun StageBlock(
    title: String,
    name: String,
    detail: String?,
    db: List<Double>?,
    stroke: StageStroke,
    outline: Color?,
    action: () -> Unit,
    controls: @Composable () -> Unit,
) {
    Row(
        Modifier.fillMaxWidth().border(KoanHairline, outline ?: KoanTheme.color(KoanTone.Rule)).padding(end = KoanSpace.xs),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        KoanButton(action, Modifier.weight(1f), kind = KoanButtonKind.Card) {
            Row(Modifier.fillMaxWidth().padding(KoanSpace.s), horizontalArrangement = Arrangement.spacedBy(KoanSpace.m), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    KoanText(title, role = KoanType.Fine, tone = KoanTone.Muted, appText = true)
                    KoanText(name, maxLines = 1)
                    if (!detail.isNullOrEmpty()) KoanText(detail, role = KoanType.Fine, tone = KoanTone.Muted, maxLines = 1)
                }
                db?.let { CurveThumb(it, stroke) }
            }
        }
        controls()
    }
}

/** An empty stage: a dashed place to choose what fills it. */
@Composable
private fun Placeholder(title: String?, prompt: String, outline: Color?, action: () -> Unit) {
    val dash = outline ?: KoanTheme.color(KoanTone.Muted)
    KoanButton(
        action,
        Modifier.fillMaxWidth().drawBehind {
            drawRect(dash, style = Stroke(1.dp.toPx(), pathEffect = PathEffect.dashPathEffect(floatArrayOf(4.dp.toPx(), 3.dp.toPx()))))
        },
        kind = KoanButtonKind.Card,
    ) {
        Column(Modifier.fillMaxWidth().padding(KoanSpace.s), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            title?.let { KoanText(it, role = KoanType.Fine, tone = KoanTone.Muted, appText = true) }
            Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.s), verticalAlignment = Alignment.CenterVertically) {
                KoanIcon(KoanIcons.add, contentDescription = null, color = KoanTheme.color(KoanTone.Accent))
                KoanText(prompt, tone = KoanTone.Accent, appText = true)
            }
        }
    }
}

/**
 * The chain in words, for the page's footer and for TalkBack. What plays:
 * the EQs switched on and not left out, and none on a correction that
 * already includes a tuning; then how each meets the correction, as the
 * core's `joined` says it for the CLI.
 */
internal fun sentence(o: DspOverview, device: String = o.device?.let { o.names[it] ?: it } ?: "This phone"): String {
    val aim = o.aim
    val baked = o.profiles.firstOrNull { it.name == o.active }?.role == DspRole.BAKED
    val eqs = if (baked) emptyList() else o.chain.filter { it.on && it.name !in o.leftOutEqs }.map { it.name }
    if (o.active == null && eqs.isEmpty()) return "$device is flat: the music plays untouched."
    val meets = o.chain.zip(o.joins).mapNotNull { (entry, meets) ->
        if (o.filtersOnly || entry.name !in eqs || aim == null) return@mapNotNull null
        when (val join = meets.join) {
            DspJoin.Matched -> "${entry.name} was made for $aim: matched."
            is DspJoin.Converted -> "${entry.name} was made for ${join.to}, so the difference from ${join.from} to ${join.to} plays first."
            is DspJoin.Refitted -> "${entry.name} was made for ${join.to}, so the correction is fitted to ${join.to} for it."
            DspJoin.Unknown -> "What ${entry.name} was made against is not set."
            null -> null
        }
    }
    val parts = listOfNotNull(
        o.active?.let { active -> aim?.let { "corrected by $active to $it" } ?: "corrected by $active" },
        eqs.takeIf { it.isNotEmpty() }?.let { "through " + listed(it) },
    )
    return (listOf("Music to $device, ${parts.joinToString(", then ")}.") + meets).joinToString(" ")
}

/** "a", "a and b", "a, b and c". */
internal fun listed(items: List<String>) = when (items.size) {
    0 -> ""
    1 -> items[0]
    else -> items.dropLast(1).joinToString(", ") + " and " + items.last()
}
