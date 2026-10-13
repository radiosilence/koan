package cc.blit.koan.ui.theme

import android.graphics.Bitmap
import android.os.Build
import androidx.compose.animation.animateColorAsState
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * How the app is drawn: the kōan theme or the platform's look, whether icons
 * sit beside labels, and the accent in force. Read through `KoanTheme`.
 */
@Immutable
data class KoanLook(
    val isOn: Boolean,
    val icons: Boolean,
    val dark: Boolean,
    val colors: KoanColors,
    /** The accent as drawn, eased between records. */
    val accent: Color,
    /** Whether the accent reaches 4.5:1 as text; where it does not, text in it is `ink`. */
    val accentReadsAsText: Boolean,
)

private val LocalKoanLook = staticCompositionLocalOf {
    KoanLook(
        isOn = true,
        icons = true,
        dark = true,
        colors = KoanColors.Dark,
        accent = KoanAccent.Mint.dark.color,
        accentReadsAsText = true,
    )
}

/**
 * The kōan theme, the site's look carried into the app (`appearance.theme =
 * "koan"`); with `koan` off, Material 3 as the platform draws it, in Material
 * You's dynamic colour where the device has it.
 *
 * Views name roles (`KoanText`, `KoanButton`, `Modifier.koanSurface`,
 * `KoanLabel`) and never a colour, a font or a corner. Each role draws the
 * theme when it is on and Material's nearest equivalent when it is off, so a
 * view carries no styling of its own in either.
 *
 * `accent` is the record playing's (`KoanAccent.of`), mint without one. It is
 * the room's tint in both looks: off, it replaces the dynamic primary only
 * when a record gives one.
 */
@Composable
fun KoanTheme(
    koan: Boolean = true,
    icons: Boolean = true,
    accent: KoanAccent? = null,
    dark: Boolean = isSystemInDarkTheme(),
    content: @Composable () -> Unit,
) {
    val colors = if (dark) KoanColors.Dark else KoanColors.Light
    val shade = (accent ?: KoanAccent.Mint).shade(dark)
    val system = if (koan) null else systemScheme(dark)
    val target = if (system != null && accent == null) system.primary else shade.color
    val tint by animateColorAsState(target, KoanMotion.settle(), label = "accent")
    val look = KoanLook(
        isOn = koan,
        icons = icons,
        dark = dark,
        colors = colors,
        accent = tint,
        accentReadsAsText = (system != null && accent == null) || shade.readsAsText,
    )
    val scheme = system?.copy(primary = tint) ?: koanScheme(colors, tint, dark)
    MaterialTheme(
        colorScheme = scheme,
        typography = if (koan) KoanTypography else MaterialTheme.typography,
        shapes = if (koan) KoanShapes else MaterialTheme.shapes,
    ) {
        CompositionLocalProvider(
            LocalKoanLook provides look,
            LocalKoanTokens provides KoanTokens(),
            LocalContentColor provides if (koan) colors.ink else scheme.onBackground,
            LocalTextStyle provides if (koan) KoanType.Body.koan else MaterialTheme.typography.bodyLarge,
            content = content,
        )
    }
}

/** The look in force, for roles and the few views that draw their own. */
object KoanTheme {
    val isOn: Boolean
        @Composable @ReadOnlyComposable
        get() = LocalKoanLook.current.isOn

    /** Whether icons are drawn beside labels ("Show icons"). The platform's look always draws them. */
    val icons: Boolean
        @Composable @ReadOnlyComposable
        get() = !LocalKoanLook.current.isOn || LocalKoanLook.current.icons

    val colors: KoanColors
        @Composable @ReadOnlyComposable
        get() = LocalKoanLook.current.colors

    val look: KoanLook
        @Composable @ReadOnlyComposable
        get() = LocalKoanLook.current

    /** A tone as a colour: the token in the theme, Material's nearest role otherwise. */
    @Composable
    @ReadOnlyComposable
    fun color(tone: KoanTone): Color {
        val look = LocalKoanLook.current
        val scheme = MaterialTheme.colorScheme
        val c = look.colors
        return when (tone) {
            KoanTone.Accent -> look.accent
            KoanTone.Ink -> if (look.isOn) c.ink else scheme.onSurface
            KoanTone.Strong -> if (look.isOn) c.strong else scheme.onSurface
            KoanTone.Muted -> if (look.isOn) c.muted else scheme.onSurfaceVariant
            KoanTone.Bad -> if (look.isOn) c.bad else scheme.error
            KoanTone.Rule -> if (look.isOn) c.rule else scheme.outlineVariant
        }
    }

    /** A tone for text: the accent falls back to `ink` where it cannot reach 4.5:1. */
    @Composable
    @ReadOnlyComposable
    fun textColor(tone: KoanTone): Color =
        if (tone == KoanTone.Accent && !LocalKoanLook.current.accentReadsAsText) color(KoanTone.Ink) else color(tone)

    /** A measure that differs between the looks. */
    @Composable
    @ReadOnlyComposable
    fun <T> metric(theme: T, system: T): T = if (isOn) theme else system

    /** A title the app writes, lowercase in the theme. Text from the library keeps its case. */
    @Composable
    @ReadOnlyComposable
    fun label(text: String): String = if (isOn) text.lowercase() else text
}

/** Square corners everywhere, controls, covers and sheets alike. */
private val KoanShapes = RoundedCornerShape(0).let { square ->
    Shapes(
        extraSmall = square,
        small = square,
        medium = square,
        large = square,
        extraLarge = square,
    )
}

@Composable
private fun systemScheme(dark: Boolean): ColorScheme {
    val context = LocalContext.current
    return when {
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.S ->
            if (dark) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
        dark -> darkColorScheme()
        else -> lightColorScheme()
    }
}

/**
 * The tokens as Material's roles, for the Material components the theme
 * keeps (dialogs, menus, text selection). Every container is `bg` or
 * `surface`: Material's tonal elevation would put a tinted card under them.
 */
private fun koanScheme(c: KoanColors, accent: Color, dark: Boolean): ColorScheme {
    val base = if (dark) darkColorScheme() else lightColorScheme()
    return base.copy(
        primary = accent,
        onPrimary = c.bg,
        primaryContainer = c.surface,
        onPrimaryContainer = c.strong,
        inversePrimary = accent,
        secondary = c.muted,
        onSecondary = c.bg,
        secondaryContainer = c.surface,
        onSecondaryContainer = c.ink,
        tertiary = accent,
        onTertiary = c.bg,
        tertiaryContainer = c.surface,
        onTertiaryContainer = c.ink,
        background = c.bg,
        onBackground = c.ink,
        surface = c.bg,
        onSurface = c.ink,
        surfaceVariant = c.surface,
        onSurfaceVariant = c.muted,
        surfaceTint = Color.Transparent,
        inverseSurface = c.strong,
        inverseOnSurface = c.bg,
        error = c.bad,
        onError = c.bg,
        errorContainer = c.surface,
        onErrorContainer = c.bad,
        outline = c.muted,
        outlineVariant = c.rule,
        scrim = Color.Black.copy(alpha = 0.4f),
        surfaceBright = c.surface,
        surfaceDim = c.bg,
        surfaceContainerLowest = c.bg,
        surfaceContainerLow = c.bg,
        surfaceContainer = c.bg,
        surfaceContainerHigh = c.surface,
        surfaceContainerHighest = c.surface,
    )
}

/**
 * `content` in a record's accent, the look around it otherwise kept: what a
 * record's own page wraps itself in. `null` keeps the accent in force.
 */
@Composable
fun KoanRecordAccent(accent: KoanAccent?, content: @Composable () -> Unit) {
    if (accent == null) return content()
    val look = LocalKoanLook.current
    KoanTheme(koan = look.isOn, icons = look.icons, accent = accent, dark = look.dark, content = content)
}

/**
 * `content` in the accent of a decoded sleeve, worked out off the main thread;
 * the accent in force until it is, and for `null`. A Coil bitmap needs
 * `allowHardware(false)` for its pixels to be read.
 */
@Composable
fun KoanRecordAccent(sleeve: Bitmap?, content: @Composable () -> Unit) {
    val accent by produceState<KoanAccent?>(null, sleeve) {
        value = sleeve?.let { withContext(Dispatchers.Default) { KoanAccent.of(it) } }
    }
    KoanRecordAccent(accent, content)
}
