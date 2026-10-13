package cc.blit.koan.theme

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.sizeIn
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp

/**
 * Text in a role and a tone. The app's own words are lowercase on screen, as
 * on the site; pass [data] for text that comes from the library or from
 * people (titles, artists, playlist names), which keeps its own case. The
 * string itself is untouched, so accessibility labels keep proper case.
 */
@Composable
fun KoanText(
    text: String,
    role: KoanRole = KoanRole.Body,
    tone: KoanTone = KoanTone.Ink,
    modifier: Modifier = Modifier,
    data: Boolean = false,
    maxLines: Int = 1,
    decoration: TextDecoration? = null,
    color: Color? = null,
) {
    Text(
        text = if (data) text else text.lowercase(),
        modifier = modifier,
        color = color ?: KoanTheme.colors.tone(tone),
        style = KoanTheme.style(role),
        maxLines = maxLines,
        overflow = TextOverflow.Ellipsis,
        textDecoration = decoration,
    )
}

/** A one-point rule in `rule`. */
@Composable
fun KoanRule(modifier: Modifier = Modifier) {
    Box(modifier.fillMaxWidth().height(KoanShape.hairline).background(KoanTheme.colors.rule))
}

/** Which weight a control carries; see the rules in `docs/design/koan-theme.md`. */
enum class KoanButtonKind {
    /** The label in `accent`, a one-point `accent` outline, `body` type. At most one per screen. */
    Prominent,

    /** The label in `ink`, no outline, `control` type. */
    Standard,

    /** As standard in `meta` type, tight padding. Repeated and secondary controls. */
    Compact,

    /** Compact, in a one-point `muted` outline: a secondary action standing in a form row. */
    Bordered,

    /** The label in `muted`, underlined; `ink` when pressed. */
    Link,

    /** Compact, in `bad`. */
    Destructive,

    /** The label in `muted`, no outline: used in bars. */
    Text,
}

/**
 * A button in one of the theme's weights. Flat and square, labels lowercase
 * and on one line. Pressed is `hover` fill; disabled is 40 % opacity. The
 * whole of it takes the tap: its outline, its padding and the cell around its
 * label.
 */
@Composable
fun KoanButton(
    label: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    kind: KoanButtonKind = KoanButtonKind.Standard,
    enabled: Boolean = true,
    data: Boolean = false,
    icon: String? = null,
) {
    val colors = KoanTheme.colors
    val source = remember { MutableInteractionSource() }
    val pressed by source.collectIsPressedAsState()
    val role = when (kind) {
        KoanButtonKind.Prominent -> KoanRole.Body
        KoanButtonKind.Standard -> KoanRole.Control
        else -> KoanRole.Meta
    }
    val tone = when (kind) {
        KoanButtonKind.Prominent -> KoanTone.Accent
        KoanButtonKind.Standard, KoanButtonKind.Compact, KoanButtonKind.Bordered -> KoanTone.Ink
        KoanButtonKind.Link, KoanButtonKind.Text -> if (pressed) KoanTone.Ink else KoanTone.Muted
        KoanButtonKind.Destructive -> KoanTone.Bad
    }
    val outline = when (kind) {
        KoanButtonKind.Prominent -> BorderStroke(KoanShape.hairline, colors.accent)
        KoanButtonKind.Bordered -> BorderStroke(KoanShape.hairline, colors.muted)
        else -> null
    }
    // Padding as the document gives it: 12 x 20 prominent, 8 x 4 standard,
    // 4 x 2 compact, 6 x 12 bordered.
    val padding = when (kind) {
        KoanButtonKind.Prominent -> Pair(20.dp, 12.dp)
        KoanButtonKind.Standard -> Pair(8.dp, 4.dp)
        KoanButtonKind.Bordered -> Pair(12.dp, 6.dp)
        else -> Pair(4.dp, 2.dp)
    }
    var m = modifier
        .sizeIn(minHeight = KoanShape.hit)
        .alpha(if (enabled) 1f else 0.4f)
        .background(if (pressed && enabled) colors.hover else Color.Transparent, RectangleShape)
    if (outline != null) m = m.border(outline, RectangleShape)
    m = m
        .clickable(source, indication = null, enabled = enabled, role = Role.Button, onClick = onClick)
        .padding(horizontal = padding.first, vertical = padding.second)
    Row(m, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.Center) {
        if (icon != null) {
            KoanIcon(icon, tone = tone)
            Box(Modifier.width(KoanSpace.s))
        }
        KoanText(
            label, role, tone, data = data,
            decoration = if (kind == KoanButtonKind.Link) TextDecoration.Underline else null,
        )
    }
}

/** A transport icon button: the glyph in `ink`, at least 44 by 44 to hit, no outline. */
@Composable
fun KoanIconButton(
    icon: String,
    description: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    tone: KoanTone = KoanTone.Ink,
    enabled: Boolean = true,
    outlined: Boolean = false,
) {
    val colors = KoanTheme.colors
    val source = remember { MutableInteractionSource() }
    val pressed by source.collectIsPressedAsState()
    var m = modifier
        .size(KoanShape.hit)
        .alpha(if (enabled) 1f else 0.4f)
        .background(if (pressed && enabled) colors.hover else Color.Transparent, RectangleShape)
    if (outlined) m = m.border(BorderStroke(KoanShape.hairline, colors.ink), RectangleShape)
    Box(
        m.clickable(source, indication = null, enabled = enabled, role = Role.Button, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        KoanIcon(icon, tone = tone, textSizeDp = 20f, description = description)
    }
}

/**
 * A square box, 14 by 14 inside a 44-point hit area. Off: a one-point `muted`
 * outline. On: filled `accent` with a `bg` check. The label leads and the box
 * sits at the row's trailing edge, where a switch would.
 */
@Composable
fun KoanToggle(
    label: String,
    checked: Boolean,
    onCheckedChange: (Boolean) -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
) {
    val colors = KoanTheme.colors
    Row(
        modifier
            .fillMaxWidth()
            .sizeIn(minHeight = KoanShape.hit)
            .alpha(if (enabled) 1f else 0.4f)
            .clickable(enabled = enabled, role = Role.Switch, indication = null, interactionSource = remember { MutableInteractionSource() }) {
                onCheckedChange(!checked)
            }
            .semantics { stateDescription = if (checked) "on" else "off" },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        KoanText(label, KoanRole.Body, modifier = Modifier.weight(1f))
        Box(
            Modifier.size(14.dp)
                .then(if (checked) Modifier.background(colors.accent) else Modifier.border(BorderStroke(KoanShape.hairline, colors.muted))),
            contentAlignment = Alignment.Center,
        ) {
            if (checked) KoanIcon("check", tone = KoanTone.Ink, textSizeDp = 10f, modifier = Modifier)
        }
    }
}

/** One tab of [KoanTabBar]. */
data class KoanTab<T>(val key: T, val label: String)

/**
 * The phone's tab bar: flat and full-width, a one-point `rule` along its top
 * and `bg` beneath, 64 points tall. Labels in `fine`, lowercase, no glyphs.
 * Unselected `muted`; selected `accent`, the label underlined. Tapping the
 * selected tab calls [onReselect], which the navigator takes as "back to the
 * root" of that tab.
 */
@Composable
fun <T> KoanTabBar(
    tabs: List<KoanTab<T>>,
    selected: T,
    onSelect: (T) -> Unit,
    onReselect: (T) -> Unit,
    modifier: Modifier = Modifier,
) {
    val colors = KoanTheme.colors
    Column(modifier.background(colors.bg)) {
        KoanRule()
        Row(Modifier.fillMaxWidth().height(KoanShape.tabBar)) {
            for (tab in tabs) {
                val isSelected = tab.key == selected
                Box(
                    Modifier
                        .weight(1f)
                        .height(KoanShape.tabBar)
                        .clickable(role = Role.Tab, indication = null, interactionSource = remember { MutableInteractionSource() }) {
                            if (isSelected) onReselect(tab.key) else onSelect(tab.key)
                        }
                        .semantics { this.selected = isSelected },
                    contentAlignment = Alignment.Center,
                ) {
                    KoanText(
                        tab.label, KoanRole.Fine,
                        if (isSelected) KoanTone.Accent else KoanTone.Muted,
                        decoration = if (isSelected) TextDecoration.Underline else null,
                    )
                }
            }
        }
    }
}
