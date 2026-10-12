package cc.blit.koan.theme

import androidx.compose.runtime.Immutable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/**
 * The colour tokens of `docs/design/koan-theme.md`, which come from
 * `site/src/theme.css`. Views name a role and never a colour: the accent is
 * the record playing, resolved by [KoanAccent] for whichever appearance is on.
 */
@Immutable
data class KoanColors(
    /** Window and page background. */
    val bg: Color,
    /** A raised field, a sheet. Used sparingly; most separation is by rule. */
    val surface: Color,
    /** Hairlines between rows and around regions. Decoration only: under 3:1. */
    val rule: Color,
    /** Pressed fill. */
    val hover: Color,
    /** Body text and icons. */
    val ink: Color,
    /** Titles and the playing track's name. */
    val strong: Color,
    /** Secondary text, unselected navigation, control outlines. */
    val muted: Color,
    /** Selection, the primary action, progress, the playing row, toggles on, focus. */
    val accent: Color,
    /** Whether [accent] reaches 4.5:1, so may be text; where not, text that would have been the accent is [ink]. */
    val accentReadsAsText: Boolean,
    /** Errors and destructive actions. */
    val bad: Color,
    val dark: Boolean,
) {
    /** The colour for a [KoanTone] as text and glyphs are drawn: the accent falls back to ink where it cannot be read. */
    fun tone(tone: KoanTone): Color = when (tone) {
        KoanTone.Ink -> ink
        KoanTone.Strong -> strong
        KoanTone.Muted -> muted
        KoanTone.Accent -> if (accentReadsAsText) accent else ink
        KoanTone.Bad -> bad
        KoanTone.Rule -> rule
    }

    companion object {
        fun of(accent: KoanAccent, dark: Boolean): KoanColors {
            val shade = accent.shade(dark)
            val tint = Color(0xFF000000.toInt() or shade.rgb)
            return if (dark) {
                KoanColors(
                    bg = Color(0xFF1E1E1E), surface = Color(0xFF2A2A2A), rule = Color(0xFF383838),
                    hover = Color(0xFF4D4D4D), ink = Color(0xFFCCCCCC), strong = Color(0xFFFFFFFF),
                    muted = Color(0xFF919191), accent = tint, accentReadsAsText = shade.readsAsText,
                    bad = Color(0xFFEF6B73), dark = true,
                )
            } else {
                KoanColors(
                    bg = Color(0xFFFFFFFF), surface = Color(0xFFF2F2F2), rule = Color(0xFFE0E0E0),
                    hover = Color(0xFFC4C4C4), ink = Color(0xFF333333), strong = Color(0xFF111111),
                    muted = Color(0xFF666666), accent = tint, accentReadsAsText = shade.readsAsText,
                    bad = Color(0xFFC43F3F), dark = false,
                )
            }
        }
    }
}

/** The colours text and glyphs are drawn in. `Rule` is for strokes and fills, never text. */
enum class KoanTone { Ink, Strong, Muted, Accent, Bad, Rule }

/** Spacing steps. Page margins are [page] on a phone. */
object KoanSpace {
    val xs: Dp = 4.dp
    val s: Dp = 8.dp
    val m: Dp = 12.dp
    val l: Dp = 16.dp
    val xl: Dp = 22.dp
    val xxl: Dp = 32.dp
    val page: Dp = xl
}

/** Shape and size constants. Corners are square: there is no radius to name. */
object KoanShape {
    /** One rule's width: a point, not a pixel. */
    val hairline: Dp = 1.dp

    /** The smallest thing that takes a tap. */
    val hit: Dp = 44.dp

    /** The flat tab bar's height, not counting the system bars. */
    val tabBar: Dp = 64.dp

    /** The rule along a selected navigation row's leading edge, and the selected tab's underline. */
    val marker: Dp = 2.dp
}

/**
 * Motion: fast and direct. States take [FAST_MS], a marker moving between
 * places [NORMAL_MS], the accent arriving with a record [SETTLE_MS]; a tab
 * switch swaps the page at once. The curve is a quick-out, with no overshoot.
 */
object KoanMotion {
    const val FAST_MS = 80
    const val NORMAL_MS = 120
    const val SETTLE_MS = 250
}
