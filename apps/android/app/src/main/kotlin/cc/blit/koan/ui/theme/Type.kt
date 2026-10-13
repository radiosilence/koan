package cc.blit.koan.ui.theme

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.runtime.Composable
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontVariation
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp
import cc.blit.koan.R

/**
 * Geist Mono, the site's own variable face, bundled as `res/font/geist_mono.ttf`.
 * One instance per weight the scale uses, each pinned on the `wght` axis.
 */
val GeistMono = FontFamily(
    listOf(200, 300, 400, 500, 700).map { weight ->
        Font(
            R.font.geist_mono,
            weight = FontWeight(weight),
            variationSettings = FontVariation.Settings(FontVariation.weight(weight)),
        )
    },
)

/**
 * The roles of the theme's type scale, in sp, so each scales with the
 * system's font size. Its tabular figures align durations and counts unaided.
 */
enum class KoanType(val size: Float, val weight: FontWeight, val lineHeight: Float) {
    Display(34f, FontWeight(200), 1.1f),
    Title(26f, FontWeight(300), 1.2f),
    TitleSmall(22f, FontWeight(300), 1.2f),
    Body(15f, FontWeight.Normal, 1.45f),
    Control(14f, FontWeight.Normal, 1.3f),
    Meta(13f, FontWeight.Normal, 1.35f),
    Fine(12f, FontWeight.Normal, 1.35f),
    ;

    /** The role in Geist Mono. */
    val koan: TextStyle
        get() = TextStyle(
            fontFamily = GeistMono,
            fontWeight = weight,
            fontSize = size.sp,
            lineHeight = lineHeight.em,
            letterSpacing = 0.sp,
        )

    /** The Material role this one stands in for, which it is in the platform's look. */
    fun material(typography: Typography): TextStyle = when (this) {
        Display -> typography.headlineLarge
        Title -> typography.headlineMedium
        TitleSmall -> typography.headlineSmall
        Body -> typography.bodyLarge
        Control -> typography.labelLarge
        Meta -> typography.bodyMedium
        Fine -> typography.bodySmall
    }

    /** The role as whichever look is on. */
    val style: TextStyle
        @Composable @ReadOnlyComposable
        get() = if (KoanTheme.isOn) koan else material(MaterialTheme.typography)
}

/**
 * The type roles as Material's `Typography`, so Material components drawn in
 * the theme (a dialog, a menu) take Geist Mono at the scale's sizes.
 */
internal val KoanTypography = Typography(
    displayLarge = KoanType.Display.koan,
    displayMedium = KoanType.Display.koan,
    displaySmall = KoanType.Display.koan,
    headlineLarge = KoanType.Display.koan,
    headlineMedium = KoanType.Title.koan,
    headlineSmall = KoanType.TitleSmall.koan,
    titleLarge = KoanType.TitleSmall.koan,
    titleMedium = KoanType.Body.koan,
    titleSmall = KoanType.Control.koan,
    bodyLarge = KoanType.Body.koan,
    bodyMedium = KoanType.Meta.koan,
    bodySmall = KoanType.Fine.koan,
    labelLarge = KoanType.Control.koan,
    labelMedium = KoanType.Meta.koan,
    labelSmall = KoanType.Fine.koan,
)
