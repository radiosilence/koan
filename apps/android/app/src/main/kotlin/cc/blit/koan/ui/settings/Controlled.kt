package cc.blit.koan.ui.settings

import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.engine.Devices
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanEdge
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import cc.blit.koan.ui.theme.koanRule
import cc.blit.koan.ui.theme.koanSurface

/** How the device being controlled can be reached. */
sealed interface Reach {
    data object Connected : Reach

    /** Being woken: by the network, a push, or a notification to tap there. */
    data class Waking(val stage: String) : Reach

    /** Asleep, and a command wakes it. */
    data object Asleep : Reach

    /** Not reachable: commands go nowhere until it is back, or this phone plays. */
    data class OutOfReach(val why: String?) : Reach
}

/** Another device this phone shows and commands, and how it can be reached. */
data class Controlled(val name: String, val reach: Reach) {
    /** What the transport says in place of where the music plays. */
    val headline
        get() = when (reach) {
            Reach.Connected -> "Playing on $name"
            is Reach.Waking -> "Waking $name…"
            Reach.Asleep -> "Playing on $name, asleep"
            is Reach.OutOfReach -> "$name is out of reach"
        }

    /** What else to say under it: why it cannot be reached, or what a command does. */
    val detail
        get() = when (reach) {
            Reach.Connected -> null
            is Reach.Waking -> null
            Reach.Asleep -> "A command wakes it."
            // The engine's reason, unless it only repeats the headline.
            is Reach.OutOfReach -> listOfNotNull(reach.why?.takeUnless { it.equals("Out of reach", ignoreCase = true) }, "Commands can't reach it.").joinToString(" ")
        }
}

/**
 * The device being controlled, if one is: `null` while this phone plays. A
 * target no longer in the list is out of reach, by its id since nothing
 * else of it is known.
 */
fun controlled(devices: Devices): Controlled? {
    val target = devices.target ?: return null
    val d = devices.devices.firstOrNull { it.id == target } ?: return Controlled("Another device", Reach.OutOfReach(null))
    val reach = when {
        d.problem != null -> Reach.OutOfReach(d.problem)
        d.waking != null -> Reach.Waking(d.waking!!)
        d.awake -> Reach.Connected
        // A phone asleep that a push wakes; anything else not awake, a
        // departed Mac among them, is out of reach.
        d.wakeable -> Reach.Asleep
        else -> Reach.OutOfReach(d.wakeFailed)
    }
    return Controlled(d.name, reach)
}

/**
 * "Playing on <device>" while this phone controls another, as iOS's mini
 * player and Now Playing say it: the device and how it can be reached, with
 * one tap back to this phone. Tapping the line opens Play on. Nothing while
 * this phone plays.
 */
@Composable
fun ControlledStrip(modifier: Modifier = Modifier, ruled: Boolean = true) {
    val app = koanApp()
    val devices by app.mirror.devices.collectAsStateWithLifecycle()
    val c = controlled(devices) ?: return
    var choosing by remember { mutableStateOf(false) }
    ControlledLine(c, back = { app.mirror.launch { controlDevice(null) } }, choose = { choosing = true }, modifier, ruled)
    if (choosing) PlayOnSheet(onDismiss = { choosing = false })
}

/** The strip's line: the device and its reach, which opens Play on, and the way back to this phone. */
@Composable
internal fun ControlledLine(c: Controlled, back: () -> Unit, choose: () -> Unit, modifier: Modifier = Modifier, ruled: Boolean = true) {
    val away = c.reach is Reach.OutOfReach
    Row(
        modifier
            .fillMaxWidth()
            .then(if (ruled) Modifier.koanSurface().koanRule(KoanEdge.Top) else Modifier)
            .padding(start = if (ruled) KoanSpace.l else 0.dp, end = KoanSpace.xs, top = KoanSpace.xs, bottom = KoanSpace.xs),
        horizontalArrangement = Arrangement.spacedBy(KoanSpace.s),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Row(
            Modifier
                .weight(1f)
                .clickable(remember { MutableInteractionSource() }, indication = null, onClickLabel = "Choose where to play", onClick = choose),
            horizontalArrangement = Arrangement.spacedBy(KoanSpace.s),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            KoanIcon(KoanIcons.devices, contentDescription = null, color = KoanTheme.color(if (away) KoanTone.Bad else KoanTone.Accent))
            Column {
                KoanText(c.headline, role = KoanType.Meta, tone = if (away) KoanTone.Bad else KoanTone.Accent, maxLines = 1)
                c.detail?.let { KoanText(it, role = KoanType.Fine, tone = KoanTone.Muted, maxLines = 2) }
            }
        }
        KoanButton("Use this phone", back, kind = KoanButtonKind.Compact)
    }
}
