package cc.blit.koan.theme

import androidx.compose.runtime.Immutable
import androidx.compose.ui.text.ExperimentalTextApi
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontVariation
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.sp
import cc.blit.koan.R

/**
 * The type scale of `docs/design/koan-theme.md`: one face, Geist Mono, in
 * seven roles. Sizes are sp, so each scales with the system's text size.
 */
enum class KoanRole(val size: TextUnit, val weight: Int, val lineHeight: Double) {
    Display(34.sp, 200, 1.1),
    Title(26.sp, 300, 1.2),
    TitleSm(22.sp, 300, 1.2),
    Body(15.sp, 400, 1.45),
    Control(14.sp, 400, 1.3),
    Meta(13.sp, 400, 1.35),
    Fine(12.sp, 400, 1.35),
}

/**
 * Geist Mono, variable in weight (100 to 900). The file is the site's own
 * `geist-mono.woff2` converted to TrueType, since Android cannot read woff2;
 * the variable axis is set per weight through [FontVariation].
 */
@Immutable
@OptIn(ExperimentalTextApi::class)
class KoanTypography {
    private val families = HashMap<Int, FontFamily>()

    private fun family(weight: Int): FontFamily = families.getOrPut(weight) {
        FontFamily(
            Font(
                R.font.geist_mono,
                weight = FontWeight(weight),
                variationSettings = FontVariation.Settings(FontVariation.weight(weight)),
            ),
        )
    }

    /** The style for a role. Figures are tabular in a monospace face, so durations and counts align. */
    fun style(role: KoanRole): TextStyle = TextStyle(
        fontFamily = family(role.weight),
        fontWeight = FontWeight(role.weight),
        fontSize = role.size,
        lineHeight = role.size * role.lineHeight,
    )
}
