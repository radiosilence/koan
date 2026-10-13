package cc.blit.koan.theme

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.sizeIn
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.input.ImeAction

/**
 * A one-line text field in the theme: `surface` fill, square, `ink` text, an
 * `accent` caret, the placeholder in `muted`. A minimal field for the browsers'
 * filter and year boxes; the theme's general field, when it lands in
 * `KoanComponents.kt`, replaces it. Clearing is a button at the trailing edge
 * once there is something to clear.
 */
@Composable
fun KoanFilterField(
    value: String,
    onValueChange: (String) -> Unit,
    placeholder: String,
    modifier: Modifier = Modifier,
    keyboardOptions: KeyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
    clearable: Boolean = true,
) {
    val colors = KoanTheme.colors
    BasicTextField(
        value = value,
        onValueChange = onValueChange,
        modifier = modifier,
        singleLine = true,
        textStyle = KoanTheme.style(KoanRole.Body).copy(color = colors.ink),
        cursorBrush = SolidColor(colors.accent),
        keyboardOptions = keyboardOptions,
        decorationBox = { inner ->
            Row(
                Modifier.fillMaxWidth().sizeIn(minHeight = KoanShape.hit).background(colors.surface)
                    .padding(start = KoanSpace.m),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Box(Modifier.weight(1f)) {
                    if (value.isEmpty()) KoanText(placeholder, KoanRole.Body, KoanTone.Muted)
                    inner()
                }
                if (clearable && value.isNotEmpty()) {
                    KoanIconButton("close", "Clear", onClick = { onValueChange("") }, tone = KoanTone.Muted)
                }
            }
        },
    )
}
