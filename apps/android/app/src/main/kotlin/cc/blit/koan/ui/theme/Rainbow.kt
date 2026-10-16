package cc.blit.koan.ui.theme

import androidx.compose.runtime.Composable
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.graphicsLayer

/**
 * Gay mode, as the Apple apps' `Rainbow`: the accent in an acid hyperpop
 * palette, cycling, and the palette through the titles that carry it.
 * Hidden on purpose: no Settings row and no docs. Seven taps on the version
 * line switch it (`appearance.rainbow`).
 *
 * Off, it costs nothing: the only timer is the accent's cycle, and that runs
 * only while it is drawn, animations are on and the app is in front.
 */
object Rainbow {
    /** Hot pink, cyber blue, chrome, holographic lilac, brat lime. */
    val hyperpop = listOf(0xFF2E9A, 0x00C8FF, 0xC9CED6, 0xB388FF, 0x8ACE00).map { Color(0xFF000000 or it.toLong()) }

    /** Chrome has no hue to carry, so it sits out of the accent's cycle; the gradients have it. */
    private val hued = hyperpop.filterIndexed { i, _ -> i != 2 }

    /** How long the accent holds each colour. */
    const val PERIOD_MS = 2_000L

    /** The accent at a place around the palette, tone-mapped as a record's would be. */
    fun accent(step: Int): KoanAccent = KoanAccent.of(hued[Math.floorMod(step, hued.size)])

    /** The palette turned to start at `step`, and closed, for a gradient that sweeps with the accent. */
    fun stops(step: Int): List<Color> {
        val turned = List(hyperpop.size) { hyperpop[Math.floorMod(step + it, hyperpop.size)] }
        return turned + turned.first()
    }

    /** What the toast says as it comes on, one at random. */
    val on = listOf(
        "gay mode: ON ✦ it's giving",
        "slay ✧˖°",
        "ur playlist ate",
        "girl what is this song 💅",
        "hyperpop mode unlocked ⋆｡°✩",
        "no thoughts just vibes",
        "mother is mothering",
        "the gays are fighting in the comments",
        "who let her cook 🍳",
        "flop era cancelled",
        "this is her renaissance",
        "chart position: iconic",
        "serving cunt, sorry, i mean content 💅",
        "ok diva",
        "pop girlies stay winning",
    )

    /** And as it goes off. */
    val off = listOf(
        "ok back to being normal i guess",
        "and the music stops… for now",
    )
}

/** Where the accent is around the palette while the rainbow is drawn; `null` while it is not. */
val LocalRainbowStep = compositionLocalOf<Int?> { null }

/**
 * A title in gay mode: the palette through its text, moving along with the
 * accent's cycle, with no clock of its own. Plain with the rainbow off.
 */
@Composable
fun Modifier.rainbowShimmer(): Modifier {
    val step = LocalRainbowStep.current ?: return this
    val brush = Brush.horizontalGradient(Rainbow.stops(step))
    return graphicsLayer(compositingStrategy = CompositingStrategy.Offscreen).drawWithContent {
        drawContent()
        drawRect(brush, blendMode = BlendMode.SrcIn)
    }
}

/**
 * Seven taps on the version line, each within a second of the last, as
 * iOS counts them. True on the seventh, and the count starts again.
 */
class SecretTaps(private val needed: Int = 7, private val withinMs: Long = 1_000) {
    private var count = 0
    private var last = Long.MIN_VALUE

    fun tap(atMs: Long): Boolean {
        count = if (last != Long.MIN_VALUE && atMs - last < withinMs) count + 1 else 1
        last = atMs
        if (count < needed) return false
        count = 0
        return true
    }
}
