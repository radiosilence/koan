package cc.blit.koan.ui.theme

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.unit.dp

/**
 * A menu as the theme draws one: square, on `bg`, with a hairline edge and
 * no shadow. Material's in the platform's look.
 */
@Composable
fun KoanMenu(expanded: Boolean, onDismissRequest: () -> Unit, modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit) {
    val on = KoanTheme.isOn
    DropdownMenu(
        expanded = expanded,
        onDismissRequest = onDismissRequest,
        modifier = modifier,
        shape = if (on) RectangleShape else MaterialTheme.shapes.extraSmall,
        containerColor = if (on) KoanTheme.colors.bg else MaterialTheme.colorScheme.surfaceContainer,
        tonalElevation = if (on) 0.dp else 3.dp,
        shadowElevation = if (on) 0.dp else 3.dp,
        border = if (on) BorderStroke(KoanHairline, KoanTheme.colors.rule) else null,
        content = content,
    )
}

/** An item of a `KoanMenu`, in the app's words: lowercased in the theme, its glyph before it. */
@Composable
fun KoanMenuItem(title: String, icon: Int? = null, destructive: Boolean = false, enabled: Boolean = true, onClick: () -> Unit) {
    val tone = if (destructive) KoanTone.Bad else KoanTone.Ink
    val colour = KoanTheme.color(tone)
    DropdownMenuItem(
        text = { if (KoanTheme.isOn) KoanText(title, role = KoanType.Control, tone = tone, appText = true) else Text(title) },
        leadingIcon = icon?.let { { KoanIcon(it, null, color = colour) } },
        onClick = onClick,
        enabled = enabled,
    )
}
