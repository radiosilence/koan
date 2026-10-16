package cc.blit.koan.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.remember
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle

/** The colours in force: the appearance's tokens with the record's accent. */
val LocalKoan = compositionLocalOf { KoanColors.of(KoanAccent.Mint, dark = true) }

/** Geist Mono, in its roles. */
val LocalKoanType = compositionLocalOf { KoanTypography() }

/** Where the theme can be read from inside a composable: `KoanTheme.colors`, `KoanTheme.style(role)`. */
object KoanTheme {
    val colors: KoanColors
        @Composable @ReadOnlyComposable get() = LocalKoan.current

    @Composable
    @ReadOnlyComposable
    fun style(role: KoanRole): TextStyle = LocalKoanType.current.style(role)
}

/**
 * The kōan theme: the tokens, the type and the square shapes, with Material 3
 * underneath as structure only. The colour tokens map to a `ColorScheme` so
 * the Material components that remain (the scaffold, text fields, sheets) draw
 * on the ground and in the ink they should, and are never relied on for the
 * look: buttons, the tab bar, toggles and rows are the theme's own.
 *
 * [accent] is the record playing (see [KoanAccent.of]); mint with nothing
 * playing. The wash and its drift are not here: they are drawn behind the
 * pages, by the page that has the room.
 */
@Composable
fun KoanTheme(
    accent: KoanAccent = KoanAccent.Mint,
    dark: Boolean = isSystemInDarkTheme(),
    content: @Composable () -> Unit,
) {
    val colors = remember(accent, dark) { KoanColors.of(accent, dark) }
    val type = remember { KoanTypography() }
    val scheme = remember(colors) { materialScheme(colors) }
    val typography = remember(type) { materialTypography(type) }
    CompositionLocalProvider(LocalKoan provides colors, LocalKoanType provides type) {
        MaterialTheme(colorScheme = scheme, typography = typography, shapes = SquareShapes, content = content)
    }
}

/** All zero radius: corners are square. */
private val SquareShapes = Shapes(
    extraSmall = RoundedCornerShape(0), small = RoundedCornerShape(0), medium = RoundedCornerShape(0),
    large = RoundedCornerShape(0), extraLarge = RoundedCornerShape(0),
)

/**
 * `bg` to `background`, `surface` to `surface`, `accent` to `primary`, `ink`
 * to `onBackground`, `muted` to `onSurfaceVariant`, `rule` to
 * `outlineVariant`, `bad` to `error`, as the theme document sets out.
 */
internal fun materialScheme(c: KoanColors) = (if (c.dark) darkColorScheme() else lightColorScheme()).copy(
    primary = c.accent, onPrimary = c.bg, primaryContainer = c.surface, onPrimaryContainer = c.ink,
    inversePrimary = c.accent,
    secondary = c.muted, onSecondary = c.bg, secondaryContainer = c.surface, onSecondaryContainer = c.ink,
    tertiary = c.accent, onTertiary = c.bg, tertiaryContainer = c.surface, onTertiaryContainer = c.ink,
    background = c.bg, onBackground = c.ink,
    surface = c.bg, onSurface = c.ink, surfaceVariant = c.surface, onSurfaceVariant = c.muted,
    surfaceTint = Color.Transparent,
    inverseSurface = c.ink, inverseOnSurface = c.bg,
    error = c.bad, onError = c.bg, errorContainer = c.surface, onErrorContainer = c.bad,
    outline = c.muted, outlineVariant = c.rule, scrim = Color.Black,
    surfaceBright = c.surface, surfaceDim = c.bg,
    surfaceContainer = c.surface, surfaceContainerHigh = c.surface, surfaceContainerHighest = c.surface,
    surfaceContainerLow = c.bg, surfaceContainerLowest = c.bg,
)

internal fun materialTypography(t: KoanTypography) = Typography(
    displayLarge = t.style(KoanRole.Display), displayMedium = t.style(KoanRole.Display),
    displaySmall = t.style(KoanRole.Title),
    headlineLarge = t.style(KoanRole.Title), headlineMedium = t.style(KoanRole.TitleSm),
    headlineSmall = t.style(KoanRole.TitleSm),
    titleLarge = t.style(KoanRole.TitleSm), titleMedium = t.style(KoanRole.Body), titleSmall = t.style(KoanRole.Control),
    bodyLarge = t.style(KoanRole.Body), bodyMedium = t.style(KoanRole.Control), bodySmall = t.style(KoanRole.Meta),
    labelLarge = t.style(KoanRole.Control), labelMedium = t.style(KoanRole.Meta), labelSmall = t.style(KoanRole.Fine),
)
