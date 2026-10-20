package cc.blit.koan.ui.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import cc.blit.koan.ffi.DspRole
import cc.blit.koan.ffi.DspTargetOption
import cc.blit.koan.ui.theme.KoanBadge
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanFormRow
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanSectionHeader
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType

/**
 * Every target, by the headphones it is for, and those added once: each
 * list of targets for an ear ends with the added ones, and a list with a row
 * twice cannot choose either. Neutral is a target of both ears, so a
 * target's ear is said by the heading it is under. `first` is whether the
 * device's correction is for in-ears, which puts its kind first and the
 * other under Other; `null` shows both as they are.
 */
data class TargetGroups(
    val over: List<DspTargetOption> = emptyList(),
    val inEar: List<DspTargetOption> = emptyList(),
    val added: List<DspTargetOption> = emptyList(),
    val first: Boolean? = null,
) {
    val isEmpty get() = over.isEmpty() && inEar.isEmpty() && added.isEmpty()

    /** The targets' ids under a heading for each group, Unknown ("") first. */
    val sections: List<Pair<String?, List<String>>>
        get() {
            val groups = when (first) {
                true -> listOf("In-ear" to inEar, "Other" to over, "Added" to added)
                false -> listOf("Over-ear" to over, "Other" to inEar, "Added" to added)
                null -> listOf("Over-ear" to over, "In-ear" to inEar, "Added" to added)
            }
            return listOf<Pair<String?, List<String>>>(null to listOf("")) +
                groups.filter { it.second.isNotEmpty() }.map { (title, targets) -> title to targets.map { it.id } }
        }

    fun option(id: String) = (over + inEar + added).firstOrNull { it.id == id }

    companion object {
        fun of(over: List<DspTargetOption>, inEar: List<DspTargetOption>, first: Boolean?): TargetGroups {
            val both = over.map { it.id }.toSet() intersect inEar.map { it.id }.toSet()
            return TargetGroups(over.filter { it.id !in both }, inEar.filter { it.id !in both }, over.filter { it.id in both }, first)
        }
    }
}

/**
 * A choice among targets, as a form row: the one chosen, opening a list of
 * them under their headings, each with what it does and anything `extra`
 * draws beside it. Chosen, the list closes.
 */
@Composable
fun TargetPickerRow(
    label: String,
    selected: String,
    sections: List<Pair<String?, List<String>>>,
    name: (String) -> String,
    onSelect: (String) -> Unit,
    describe: (String) -> String = { "" },
    extra: @Composable (String) -> Unit = {},
) {
    var open by remember { mutableStateOf(false) }
    KoanFormRow(label, inset) {
        KoanButton({ open = true }, kind = KoanButtonKind.Bordered) {
            Text(name(selected), maxLines = 1, overflow = TextOverflow.Ellipsis)
            KoanIcon(KoanIcons.choose, contentDescription = null)
        }
    }
    if (!open) return
    AlertDialog(
        onDismissRequest = { open = false },
        title = { KoanText(label, role = KoanType.TitleSmall, appText = true) },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(KoanSpace.xs)) {
                sections.forEach { (title, ids) ->
                    title?.let { KoanSectionHeader(it) }
                    ids.forEach { id ->
                        KoanButton({
                            open = false
                            if (id != selected) onSelect(id)
                        }, Modifier.fillMaxWidth(), kind = KoanButtonKind.Card) {
                            Row(
                                Modifier.fillMaxWidth().padding(vertical = KoanSpace.xs),
                                horizontalArrangement = Arrangement.spacedBy(KoanSpace.m),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                Column(Modifier.weight(1f)) {
                                    KoanText(name(id), tone = if (id == selected) KoanTone.Accent else KoanTone.Ink)
                                    describe(id).takeIf { it.isNotEmpty() }?.let { KoanText(it, role = KoanType.Fine, tone = KoanTone.Muted) }
                                }
                                extra(id)
                                if (id == selected) KoanIcon(KoanIcons.check, contentDescription = "Chosen", color = KoanTheme.color(KoanTone.Accent))
                            }
                        }
                    }
                }
            }
        },
        confirmButton = { KoanButton("Cancel", { open = false }) },
    )
}

/** A role's badge: beside a layer, in the chain's summary. The accent is the correction's, as on the graph. */
@Composable
fun RoleTag(role: DspRole) = KoanBadge(role.label, accent = role == DspRole.CORRECTION)

/**
 * What a profile is for, in a line each, at the top of its page: the
 * headphone the chain corrects and how, and the tunings on top.
 */
@Composable
fun ChainSummaryCard(corrects: String?, baked: Boolean, tunings: List<String>, modifier: Modifier = Modifier) {
    Column(modifier, verticalArrangement = Arrangement.spacedBy(KoanSpace.xs)) {
        Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.s), verticalAlignment = Alignment.CenterVertically) {
            RoleTag(if (baked) DspRole.BAKED else DspRole.CORRECTION)
            KoanText(corrects ?: "None", tone = if (corrects == null) KoanTone.Muted else KoanTone.Ink)
        }
        if (tunings.isNotEmpty()) {
            Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.s), verticalAlignment = Alignment.CenterVertically) {
                RoleTag(DspRole.TUNING)
                KoanText(tunings.joinToString(", "))
            }
        }
    }
}
