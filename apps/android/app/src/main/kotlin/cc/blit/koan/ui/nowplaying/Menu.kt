package cc.blit.koan.ui.nowplaying

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.unit.dp
import cc.blit.koan.ui.theme.KoanHairline
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType

/** A menu as `KoanPicker` draws its own: square, on `bg`, a hairline edge, no shadow. Material's otherwise. */
@Composable
fun KoanMenu(open: Boolean, onDismiss: () -> Unit, content: @Composable ColumnScope.() -> Unit) {
    val on = KoanTheme.isOn
    DropdownMenu(
        expanded = open,
        onDismissRequest = onDismiss,
        shape = if (on) RectangleShape else MaterialTheme.shapes.extraSmall,
        containerColor = if (on) KoanTheme.colors.bg else MaterialTheme.colorScheme.surfaceContainer,
        tonalElevation = if (on) 0.dp else 3.dp,
        shadowElevation = if (on) 0.dp else 3.dp,
        border = if (on) BorderStroke(KoanHairline, KoanTheme.colors.rule) else null,
        content = content,
    )
}

/** One choice in a `KoanMenu`: the app's words, lowercased in the theme. */
@Composable
fun KoanMenuItem(label: String, onClick: () -> Unit, icon: Int? = null, destructive: Boolean = false, enabled: Boolean = true) {
    val tone = if (destructive) KoanTone.Bad else KoanTone.Ink
    DropdownMenuItem(
        text = {
            if (KoanTheme.isOn) KoanText(label, role = KoanType.Control, tone = tone, appText = true) else Text(label)
        },
        onClick = onClick,
        enabled = enabled,
        leadingIcon = icon?.let { { KoanIcon(it, contentDescription = null, color = KoanTheme.color(tone)) } },
    )
}
