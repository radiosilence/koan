package cc.blit.koan.ui.theme

import android.os.Build
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/**
 * The kōan theme's tokens. A placeholder for the theme the Apple apps draw
 * (`docs/design/koan-theme.md`); pages read spacing and sizes from here rather
 * than writing numbers, so the theme can change them in one place.
 */
@Immutable
data class KoanTokens(
    val gutter: Dp = 16.dp,
    val rowGap: Dp = 8.dp,
    val thumb: Dp = 48.dp,
    val miniPlayerHeight: Dp = 64.dp,
)

val LocalKoanTokens = staticCompositionLocalOf { KoanTokens() }

@Composable
fun KoanTheme(content: @Composable () -> Unit) {
    val dark = isSystemInDarkTheme()
    val context = LocalContext.current
    val colours = when {
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.S ->
            if (dark) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
        dark -> darkColorScheme()
        else -> lightColorScheme()
    }
    CompositionLocalProvider(LocalKoanTokens provides KoanTokens()) {
        MaterialTheme(colorScheme = colours, content = content)
    }
}
