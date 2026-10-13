package cc.blit.koan.ui.theme

import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.TweenSpec
import androidx.compose.animation.core.tween
import androidx.compose.runtime.Immutable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp

/**
 * The theme's colour tokens in one appearance. Values and uses are set down in
 * `docs/design/koan-theme.md`; `accent` is not here, since it follows the
 * record playing (`KoanAccent`).
 */
@Immutable
data class KoanColors(
    val bg: Color,
    val surface: Color,
    val rule: Color,
    val hover: Color,
    val ink: Color,
    val strong: Color,
    val muted: Color,
    val bad: Color,
) {
    /** The rule between rows: `ink` at low opacity, so it takes on the wash beneath it. */
    val rowRule: Color get() = ink.copy(alpha = ROW_RULE_OPACITY)

    companion object {
        const val ROW_RULE_OPACITY = 0.12f

        val Dark = KoanColors(
            bg = Color(0xFF1E1E1E),
            surface = Color(0xFF2A2A2A),
            rule = Color(0xFF383838),
            hover = Color(0xFF4D4D4D),
            ink = Color(0xFFCCCCCC),
            strong = Color(0xFFFFFFFF),
            muted = Color(0xFF919191),
            bad = Color(0xFFEF6B73),
        )

        val Light = KoanColors(
            bg = Color(0xFFFFFFFF),
            surface = Color(0xFFF2F2F2),
            rule = Color(0xFFE0E0E0),
            hover = Color(0xFFC4C4C4),
            ink = Color(0xFF333333),
            strong = Color(0xFF111111),
            muted = Color(0xFF666666),
            bad = Color(0xFFC43F3F),
        )
    }
}

/** The colours text and glyphs are drawn in. `Rule` is for strokes and fills, never text. */
enum class KoanTone { Ink, Strong, Muted, Accent, Bad, Rule }

/** Spacing steps. Page margins are `xl` on a phone. */
object KoanSpace {
    val xs = 4.dp
    val s = 8.dp
    val m = 12.dp
    val l = 16.dp
    val xl = 22.dp
    val xxl = 32.dp
    val page = xl
}

/** One rule's width. */
val KoanHairline = 1.dp

/**
 * Motion: fast and direct. A quick-out curve, a snappy start and a decisive
 * stop, with no tail, overshoot or delay. States (pressed, selection, toggles)
 * take `fast`; a marker moving between places takes `normal`; the accent
 * arriving with a record takes `settle`. Compose scales every duration by the
 * system's animator scale, so with animations off all of it is instant.
 */
object KoanMotion {
    private val quickOut = CubicBezierEasing(0.2f, 0.9f, 0.3f, 1f)

    fun <T> fast(): TweenSpec<T> = tween(80, easing = quickOut)
    fun <T> normal(): TweenSpec<T> = tween(120, easing = quickOut)
    fun <T> settle(): TweenSpec<T> = tween(250, easing = quickOut)
}
