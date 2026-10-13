package cc.blit.koan.ui.theme

import androidx.compose.animation.animateColorAsState
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.drag
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.sizeIn
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.IconButton
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Slider
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.progressBarRangeInfo
import androidx.compose.ui.semantics.ProgressBarRangeInfo
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.setProgress
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/**
 * How much a control matters on its screen, as headings do for type: one
 * prominent action per screen, the rest standard, and the small actions
 * beside a row compact.
 */
enum class KoanButtonKind {
    /** The main thing done here: in the accent, in a square outline, `body` type. One per screen. */
    Prominent,

    /** The label in ink, no outline; the default. */
    Standard,

    /** Smaller and tighter, for actions beside a row or a title: favourite, ⋯, revoke. */
    Compact,

    /** Compact, in a hairline `muted` outline: a secondary action standing in a form row. */
    Bordered,

    /** `muted` and underlined, ink when pressed: it goes somewhere, or is a lesser action inline with text. */
    Link,

    /** `muted`, no outline, ink when pressed: a bar's actions ("clear", "sleep"). */
    Text,

    /** A glyph alone, in a 48-dp hit area. */
    Icon,

    /** Play and pause: a glyph in a square `ink` outline. */
    IconOutlined,

    /** A thing from the library (a record, a person): its own content and type, with the pressed fill and focus ring. */
    Card,
    ;

    internal val setsType get() = this in setOf(Prominent, Standard, Compact, Bordered, Link, Text)

    internal val type
        get() = when (this) {
            Prominent -> KoanType.Body
            Compact, Bordered -> KoanType.Meta
            else -> KoanType.Control
        }

    internal val padding
        get() = when (this) {
            Prominent -> PaddingValues(horizontal = 20.dp, vertical = 12.dp)
            Standard -> PaddingValues(horizontal = 4.dp, vertical = 8.dp)
            Compact -> PaddingValues(horizontal = 2.dp, vertical = 4.dp)
            Bordered -> PaddingValues(horizontal = 12.dp, vertical = 6.dp)
            Text, Link -> PaddingValues(vertical = 4.dp)
            IconOutlined -> PaddingValues(7.dp)
            Icon, Card -> PaddingValues(0.dp)
        }

    /** A finger's target, for the kinds too small to be one on their own. */
    internal val hit get() = if (this in setOf(Icon, IconOutlined, Compact, Link)) 48.dp else 0.dp
}

/**
 * One of the theme's buttons: square, outlined or bare, pressed to `hover`,
 * with no ripple and no motion beyond the pressed fill. In the platform's
 * look, the Material button nearest the kind.
 *
 * Words in `content` are lowercased by whoever writes them (`KoanLabel`,
 * `KoanText(appText = true)`); the overload taking a `text` does it.
 */
@Composable
fun KoanButton(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    kind: KoanButtonKind = KoanButtonKind.Standard,
    enabled: Boolean = true,
    destructive: Boolean = false,
    content: @Composable RowScope.() -> Unit,
) {
    if (!KoanTheme.isOn) {
        SystemButton(onClick, modifier, kind, enabled, destructive, content)
        return
    }
    val look = KoanTheme.look
    val c = look.colors
    val interaction = remember { MutableInteractionSource() }
    val pressed by interaction.collectIsPressedAsState()
    val focused by interaction.collectIsFocusedAsState()
    val ground by animateColorAsState(if (pressed) c.hover else Color.Transparent, KoanMotion.fast(), label = "pressed")
    val foreground = when {
        destructive && kind != KoanButtonKind.Card -> c.bad
        kind == KoanButtonKind.Prominent -> KoanTheme.textColor(KoanTone.Accent)
        kind == KoanButtonKind.Text || kind == KoanButtonKind.Link -> if (pressed) c.ink else c.muted
        else -> c.ink
    }
    val outline = when (kind) {
        KoanButtonKind.Prominent -> look.accent
        KoanButtonKind.IconOutlined -> c.ink
        KoanButtonKind.Bordered -> c.muted
        else -> null
    }
    Row(
        modifier
            .koanFocusRing(focused)
            .alpha(if (enabled) 1f else 0.4f)
            .sizeIn(minWidth = kind.hit, minHeight = kind.hit)
            .background(ground)
            .then(if (outline != null) Modifier.border(KoanHairline, outline) else Modifier)
            .clickable(interaction, indication = null, enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(kind.padding),
        horizontalArrangement = Arrangement.spacedBy(KoanSpace.s, Alignment.CenterHorizontally),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val base = if (kind.setsType) kind.type.koan else LocalTextStyle.current
        val style = base.copy(color = foreground, textDecoration = if (kind == KoanButtonKind.Link) TextDecoration.Underline else null)
        CompositionLocalProvider(LocalContentColor provides foreground, LocalTextStyle provides style) {
            content()
        }
    }
}

/** A button whose label is the app's words, lowercased in the theme, with its icon as `KoanLabel` draws it. */
@Composable
fun KoanButton(
    text: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    kind: KoanButtonKind = KoanButtonKind.Standard,
    icon: Int? = null,
    enabled: Boolean = true,
    destructive: Boolean = false,
) {
    KoanButton(onClick, modifier, kind, enabled, destructive) {
        val compact = kind == KoanButtonKind.Icon || kind == KoanButtonKind.IconOutlined
        when {
            icon != null -> KoanLabel(text, icon, style = if (compact) KoanLabelStyle.Compact else KoanLabelStyle.Full)
            else -> Text(KoanTheme.label(text), maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

@Composable
private fun SystemButton(
    onClick: () -> Unit,
    modifier: Modifier,
    kind: KoanButtonKind,
    enabled: Boolean,
    destructive: Boolean,
    content: @Composable RowScope.() -> Unit,
) {
    val error = MaterialTheme.colorScheme.error
    when (kind) {
        KoanButtonKind.Prominent -> Button(
            onClick, modifier, enabled,
            colors = if (destructive) ButtonDefaults.buttonColors(containerColor = error) else ButtonDefaults.buttonColors(),
            content = content,
        )
        KoanButtonKind.Bordered -> OutlinedButton(
            onClick, modifier, enabled,
            colors = if (destructive) ButtonDefaults.outlinedButtonColors(contentColor = error) else ButtonDefaults.outlinedButtonColors(),
            content = content,
        )
        KoanButtonKind.Icon -> IconButton(onClick, modifier, enabled) { Row(content = content) }
        KoanButtonKind.IconOutlined -> FilledIconButton(onClick, modifier, enabled) { Row(content = content) }
        KoanButtonKind.Card -> Row(modifier.clickable(enabled = enabled, onClick = onClick), content = content)
        else -> TextButton(
            onClick, modifier, enabled,
            colors = if (destructive) ButtonDefaults.textButtonColors(contentColor = error) else ButtonDefaults.textButtonColors(),
            content = content,
        )
    }
}

/**
 * A toggle as the theme draws it: the label leading in `body`, and a square
 * 14-dp box trailing where a switch would sit, a `muted` outline off and
 * filled with the accent and checked in `bg` on. Material's switch otherwise.
 */
@Composable
fun KoanToggle(label: String, checked: Boolean, onCheckedChange: (Boolean) -> Unit, modifier: Modifier = Modifier, enabled: Boolean = true) {
    val on = KoanTheme.isOn
    Row(
        modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .alpha(if (enabled || !on) 1f else 0.4f)
            .toggleable(checked, enabled = enabled, role = Role.Switch, onValueChange = onCheckedChange),
        horizontalArrangement = Arrangement.spacedBy(KoanSpace.m),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.weight(1f)) { KoanText(label, appText = true) }
        if (on) {
            val c = KoanTheme.colors
            val fill by animateColorAsState(if (checked) KoanTheme.look.accent else Color.Transparent, KoanMotion.fast(), label = "toggle")
            Box(
                Modifier
                    .size(14.dp)
                    .background(fill)
                    .then(if (checked) Modifier else Modifier.border(KoanHairline, c.muted)),
                contentAlignment = Alignment.Center,
            ) {
                if (checked) KoanIcon(KoanIcons.check, contentDescription = null, size = with(LocalDensity.current) { 9.dp.toSp() }, color = c.bg)
            }
        } else {
            Switch(checked, onCheckedChange = null, enabled = enabled)
        }
    }
}

/**
 * A segmented control as the theme draws it: options as text in `control`
 * type, 18 apart; the chosen one in `ink`, underlined in the accent, the rest
 * `muted`. Material's segmented buttons otherwise.
 */
@Composable
fun <T> KoanSegmented(
    options: List<Pair<String, T>>,
    selected: T,
    onSelect: (T) -> Unit,
    modifier: Modifier = Modifier,
) {
    if (!KoanTheme.isOn) {
        SingleChoiceSegmentedButtonRow(modifier) {
            options.forEachIndexed { index, (label, value) ->
                SegmentedButton(
                    selected = value == selected,
                    onClick = { onSelect(value) },
                    shape = SegmentedButtonDefaults.itemShape(index, options.size),
                ) { Text(label) }
            }
        }
        return
    }
    val c = KoanTheme.colors
    val accent = KoanTheme.look.accent
    Row(modifier, horizontalArrangement = Arrangement.spacedBy(18.dp)) {
        for ((label, value) in options) {
            val chosen = value == selected
            val colour by animateColorAsState(if (chosen) c.ink else c.muted, KoanMotion.fast(), label = "segment")
            Box(
                Modifier
                    .heightIn(min = 48.dp)
                    .selectable(chosen, role = Role.Tab, onClick = { onSelect(value) }),
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    KoanTheme.label(label),
                    Modifier
                        .padding(bottom = 5.dp)
                        .drawBehind {
                            if (chosen) {
                                val w = KoanHairline.toPx()
                                drawRect(accent, Offset(0f, size.height - w), Size(size.width, w))
                            }
                        },
                    style = KoanType.Control.koan,
                    color = colour,
                    maxLines = 1,
                )
            }
        }
    }
}

/**
 * A slider as the theme draws it: a 1-dp `rule` track, a 3-dp accent fill up
 * to the value, and a square 8-dp `ink` thumb shown only while dragged or
 * focused, in a 48-dp hit area. Values sit at either end in `fine`, `muted`.
 * Material's slider otherwise.
 */
@Composable
fun KoanSlider(
    value: Float,
    onValueChange: (Float) -> Unit,
    modifier: Modifier = Modifier,
    range: ClosedFloatingPointRange<Float> = 0f..1f,
    onValueChangeFinished: () -> Unit = {},
    enabled: Boolean = true,
    low: (@Composable () -> Unit)? = null,
    high: (@Composable () -> Unit)? = null,
) {
    Row(modifier, horizontalArrangement = Arrangement.spacedBy(KoanSpace.s), verticalAlignment = Alignment.CenterVertically) {
        ProvideKoanType(KoanType.Fine, KoanTheme.color(KoanTone.Muted)) { low?.invoke() }
        if (KoanTheme.isOn) {
            KoanTrack(value, onValueChange, Modifier.weight(1f), range, onValueChangeFinished, enabled)
        } else {
            Slider(value, onValueChange, Modifier.weight(1f), enabled, range, onValueChangeFinished = onValueChangeFinished)
        }
        ProvideKoanType(KoanType.Fine, KoanTheme.color(KoanTone.Muted)) { high?.invoke() }
    }
}

@Composable
private fun KoanTrack(
    value: Float,
    onValueChange: (Float) -> Unit,
    modifier: Modifier,
    range: ClosedFloatingPointRange<Float>,
    finished: () -> Unit,
    enabled: Boolean,
) {
    val c = KoanTheme.colors
    val accent = KoanTheme.look.accent
    val change by rememberUpdatedState(onValueChange)
    val done by rememberUpdatedState(finished)
    var dragging by remember { mutableStateOf(false) }
    var focused by remember { mutableStateOf(false) }
    val span = range.endInclusive - range.start
    val fraction = if (span > 0) ((value - range.start) / span).coerceIn(0f, 1f) else 0f
    Box(
        modifier
            .height(48.dp)
            .alpha(if (enabled) 1f else 0.4f)
            .onFocusChanged { focused = it.isFocused }
            .semantics {
                progressBarRangeInfo = ProgressBarRangeInfo(value, range)
                if (enabled) {
                    setProgress { target ->
                        change(target.coerceIn(range))
                        done()
                        true
                    }
                }
            }
            .then(
                if (!enabled) Modifier else Modifier.pointerInput(range) {
                    fun at(x: Float) = range.start + span * (x / size.width).coerceIn(0f, 1f)
                    awaitEachGesture {
                        val down = awaitFirstDown()
                        dragging = true
                        change(at(down.position.x))
                        drag(down.id) {
                            change(at(it.position.x))
                            it.consume()
                        }
                        dragging = false
                        done()
                    }
                },
            )
            .drawBehind {
                val mid = size.height / 2
                val rule = KoanHairline.toPx()
                val x = size.width * fraction
                drawRect(c.rule, Offset(0f, mid - rule / 2), Size(size.width, rule))
                val fill = 3.dp.toPx()
                drawRect(accent, Offset(0f, mid - fill / 2), Size(x, fill))
                if (dragging || focused) {
                    val thumb = 8.dp.toPx()
                    drawRect(c.ink, Offset((x - thumb / 2).coerceIn(0f, size.width - thumb), mid - thumb / 2), Size(thumb, thumb))
                }
            },
    )
}

/**
 * A text field: the theme's type on a square `surface` field with no bezel,
 * or on nothing at all without `ground`. Material's outlined field otherwise.
 */
@Composable
fun KoanField(
    value: String,
    onValueChange: (String) -> Unit,
    modifier: Modifier = Modifier,
    placeholder: String = "",
    ground: Boolean = true,
    singleLine: Boolean = true,
) {
    if (!KoanTheme.isOn) {
        OutlinedTextField(value, onValueChange, modifier, singleLine = singleLine, placeholder = { Text(placeholder) })
        return
    }
    val c = KoanTheme.colors
    BasicTextField(
        value,
        onValueChange,
        modifier
            .heightIn(min = 48.dp)
            .then(if (ground) Modifier.background(c.surface) else Modifier),
        textStyle = KoanType.Body.koan.copy(color = c.ink),
        singleLine = singleLine,
        cursorBrush = SolidColor(KoanTheme.look.accent),
        decorationBox = { field ->
            Box(Modifier.padding(horizontal = KoanSpace.m, vertical = KoanSpace.s), contentAlignment = Alignment.CenterStart) {
                if (value.isEmpty()) Text(placeholder, style = KoanType.Body.koan, color = c.muted, maxLines = 1)
                field()
            }
        },
    )
}

/**
 * A pop-up choice: in the theme, the chosen option in `control`, `ink`, with
 * a chevron, in the square bordered button, opening a square `bg` panel in a
 * hairline `rule`. Options the app writes are lowercased; `keepsCase` keeps
 * options that are data, such as device names. Material's text button and
 * menu otherwise.
 */
@Composable
fun <T> KoanPicker(
    options: List<Pair<String, T>>,
    selected: T,
    onSelect: (T) -> Unit,
    modifier: Modifier = Modifier,
    keepsCase: Boolean = false,
) {
    var open by remember { mutableStateOf(false) }
    val on = KoanTheme.isOn
    fun shown(label: String) = if (keepsCase || !on) label else label.lowercase()
    val current = options.firstOrNull { it.second == selected }?.first.orEmpty()
    Box(modifier) {
        KoanButton(onClick = { open = true }, kind = KoanButtonKind.Bordered) {
            Text(shown(current), maxLines = 1, overflow = TextOverflow.Ellipsis)
            KoanIcon(KoanIcons.choose, contentDescription = null)
        }
        DropdownMenu(
            expanded = open,
            onDismissRequest = { open = false },
            shape = if (on) RectangleShape else MaterialTheme.shapes.extraSmall,
            containerColor = if (on) KoanTheme.colors.bg else MaterialTheme.colorScheme.surfaceContainer,
            tonalElevation = if (on) 0.dp else 3.dp,
            shadowElevation = if (on) 0.dp else 3.dp,
            border = if (on) BorderStroke(KoanHairline, KoanTheme.colors.rule) else null,
        ) {
            for ((label, value) in options) {
                DropdownMenuItem(
                    text = {
                        if (on) {
                            KoanText(shown(label), role = KoanType.Control, tone = if (value == selected) KoanTone.Accent else KoanTone.Ink)
                        } else {
                            Text(label)
                        }
                    },
                    onClick = {
                        onSelect(value)
                        open = false
                    },
                    trailingIcon = if (value == selected) ({ KoanIcon(KoanIcons.check, contentDescription = null) }) else null,
                )
            }
        }
    }
}

/**
 * A navigation row, as a sidebar's: the label in `body`, `muted`, after its
 * glyph; where you are, in the accent with a 2-dp accent rule on its leading
 * edge, and no fill. A plain clickable row in the platform's look, the
 * selected one in the primary colour.
 */
@Composable
fun KoanNavRow(label: String, icon: Int, selected: Boolean, onClick: () -> Unit, modifier: Modifier = Modifier) {
    val accent = KoanTheme.look.accent
    val tone = if (selected) KoanTone.Accent else KoanTone.Muted
    Row(
        modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .selectable(selected, role = Role.Tab, onClick = onClick)
            .drawBehind {
                if (selected) drawRect(accent, Offset.Zero, Size(2.dp.toPx(), size.height))
            }
            .padding(horizontal = KoanSpace.l),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        ProvideKoanType(KoanType.Body, KoanTheme.textColor(tone)) {
            KoanLabel(label, icon)
        }
    }
}

/**
 * The phone's tab bar: in the theme flat and full-width, `bg` beneath and a
 * hairline `rule` along its top, 64 dp tall, its items words alone. Material's
 * navigation bar otherwise.
 */
@Composable
fun KoanTabBar(modifier: Modifier = Modifier, content: @Composable RowScope.() -> Unit) {
    if (KoanTheme.isOn) {
        Row(
            modifier.fillMaxWidth().height(64.dp).koanSurface().koanRule(KoanEdge.Top),
            verticalAlignment = Alignment.CenterVertically,
            content = content,
        )
    } else {
        NavigationBar(modifier, content = content)
    }
}

/**
 * One tab of `KoanTabBar`: its label in `fine`, lowercase, `muted`, or in the
 * accent and underlined when chosen. Material's item, with its icon, otherwise.
 */
@Composable
fun RowScope.KoanTabItem(title: String, selected: Boolean, onClick: () -> Unit, icon: Int? = null) {
    if (!KoanTheme.isOn) {
        NavigationBarItem(
            selected = selected,
            onClick = onClick,
            icon = { icon?.let { KoanIcon(it, contentDescription = null, size = 20.sp) } },
            label = { Text(title) },
        )
        return
    }
    val accent = KoanTheme.look.accent
    val colour by animateColorAsState(KoanTheme.textColor(if (selected) KoanTone.Accent else KoanTone.Muted), KoanMotion.fast(), label = "tab")
    Column(
        Modifier
            .weight(1f)
            .heightIn(min = 48.dp)
            .selectable(selected, role = Role.Tab, onClick = onClick)
            .semantics { contentDescription = title },
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Text(
            title.lowercase(),
            Modifier
                .padding(bottom = 3.dp)
                .drawBehind {
                    if (selected) {
                        val w = KoanHairline.toPx()
                        drawRect(accent, Offset(0f, size.height - w), Size(size.width, w))
                    }
                },
            style = KoanType.Fine.koan,
            color = colour,
            maxLines = 1,
        )
    }
}

/** A form row whose label leads in `body`, `ink`, lowercase, and whose value trails in `control`, `muted`. */
@Composable
fun KoanFormRow(label: String, modifier: Modifier = Modifier, value: @Composable () -> Unit) {
    Row(
        modifier.fillMaxWidth().heightIn(min = 48.dp),
        horizontalArrangement = Arrangement.spacedBy(KoanSpace.m),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.width(120.dp)) { KoanText(label, appText = true, maxLines = 1) }
        Box(Modifier.weight(1f)) {
            ProvideKoanType(KoanType.Control, KoanTheme.color(KoanTone.Muted), value)
        }
    }
}
