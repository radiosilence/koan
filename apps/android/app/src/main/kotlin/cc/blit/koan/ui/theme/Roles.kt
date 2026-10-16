package cc.blit.koan.ui.theme

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/**
 * Text in a role of the type scale and a tone. In the platform's look, the
 * role's Material style and the nearest Material colour.
 *
 * `appText` marks words the app writes (headings, labels), which the theme
 * lowercases on screen; TalkBack still reads them as written. Text from the
 * library or from people keeps its own case.
 */
@Composable
fun KoanText(
    text: String,
    modifier: Modifier = Modifier,
    role: KoanType = KoanType.Body,
    tone: KoanTone = KoanTone.Ink,
    appText: Boolean = false,
    maxLines: Int = Int.MAX_VALUE,
    overflow: TextOverflow = TextOverflow.Ellipsis,
    textAlign: TextAlign? = null,
) {
    val shown = if (appText) KoanTheme.label(text) else text
    Text(
        shown,
        modifier = if (shown != text) modifier.semantics { contentDescription = text } else modifier,
        style = role.style,
        color = KoanTheme.textColor(tone),
        maxLines = maxLines,
        overflow = overflow,
        textAlign = textAlign,
    )
}

enum class KoanSurface { Bg, Surface }

/**
 * The ground a region sits on: `bg`, or `surface` for a raised field, used
 * sparingly. Nothing in the platform's look, which has its own.
 */
@Composable
fun Modifier.koanSurface(surface: KoanSurface = KoanSurface.Bg): Modifier =
    if (KoanTheme.isOn) {
        background(if (surface == KoanSurface.Bg) KoanTheme.colors.bg else KoanTheme.colors.surface)
    } else {
        this
    }

enum class KoanEdge { Top, Bottom, Start, End }

/** A hairline `rule` along one edge, inset from the start. Nothing in the platform's look. */
@Composable
fun Modifier.koanRule(edge: KoanEdge = KoanEdge.Bottom, inset: Dp = 0.dp, color: Color = KoanTheme.colors.rule): Modifier {
    if (!KoanTheme.isOn) return this
    return drawWithContent {
        drawContent()
        val w = KoanHairline.toPx()
        val start = inset.toPx()
        when (edge) {
            KoanEdge.Top -> drawRect(color, Offset(start, 0f), Size(size.width - start, w))
            KoanEdge.Bottom -> drawRect(color, Offset(start, size.height - w), Size(size.width - start, w))
            KoanEdge.Start -> drawRect(color, Offset.Zero, Size(w, size.height))
            KoanEdge.End -> drawRect(color, Offset(size.width - w, 0f), Size(w, size.height))
        }
    }
}

/**
 * A list row's ground: `surface` when selected, and nothing between rows,
 * which are told apart by their rhythm. Nothing in the platform's look.
 */
@Composable
fun Modifier.koanRow(selected: Boolean = false): Modifier =
    if (KoanTheme.isOn && selected) background(KoanTheme.colors.surface) else this

/**
 * Focus from a keyboard or a remote, as the theme shows it: a 2-dp ring in
 * the accent, `gap` outside the control. No lift and no shadow.
 */
@Composable
fun Modifier.koanFocusRing(on: Boolean, gap: Dp = 4.dp): Modifier {
    if (!on) return this
    val accent = KoanTheme.look.accent
    return drawWithContent {
        drawContent()
        val g = gap.toPx()
        val w = 2.dp.toPx()
        drawRect(
            accent,
            topLeft = Offset(-g - w / 2, -g - w / 2),
            size = Size(size.width + 2 * g + w, size.height + 2 * g + w),
            style = Stroke(w),
        )
    }
}

/** A heading over a group of rows: `fine`, `ink`, lowercase, with room above. */
@Composable
fun KoanSectionHeader(title: String, modifier: Modifier = Modifier) {
    if (KoanTheme.isOn) {
        KoanText(
            title,
            modifier.padding(top = KoanSpace.l).semantics { heading() },
            role = KoanType.Fine,
            appText = true,
        )
    } else {
        Text(
            title,
            modifier.padding(top = KoanSpace.l).semantics { heading() },
            style = MaterialTheme.typography.titleSmall,
            color = MaterialTheme.colorScheme.primary,
        )
    }
}

/** A hairline across a region. Material's divider in the platform's look. */
@Composable
fun KoanDivider(modifier: Modifier = Modifier) {
    if (KoanTheme.isOn) {
        Box(modifier.fillMaxWidth().height(KoanHairline).background(KoanTheme.colors.rule))
    } else {
        HorizontalDivider(modifier)
    }
}

/** How much of a `KoanLabel` shows when icons are on. */
enum class KoanLabelStyle {
    /** The title, with the icon before it. */
    Full,

    /** The icon alone, where space is a bar's; the title names it to TalkBack, and stands in for it when icons are off. */
    Compact,
}

/**
 * A label the app writes, with its icon: what navigation rows, tabs, actions
 * and bar buttons are made of. In the theme, lowercase, with its icon drawn
 * or left out as "Show icons" says; every icon-or-not decision is made here.
 * Type and colour come from around it.
 */
@Composable
fun KoanLabel(title: String, icon: Int, modifier: Modifier = Modifier, style: KoanLabelStyle = KoanLabelStyle.Full) {
    val icons = KoanTheme.icons
    val shown = KoanTheme.label(title)
    when {
        icons && style == KoanLabelStyle.Compact -> KoanIcon(icon, contentDescription = title, modifier = modifier)
        icons -> Row(
            modifier.semantics(mergeDescendants = true) { contentDescription = title },
            horizontalArrangement = Arrangement.spacedBy(KoanSpace.s),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            KoanIcon(icon, contentDescription = null)
            Text(shown, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        else -> Text(
            shown,
            modifier.semantics { contentDescription = title },
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/**
 * A small fact set apart, such as a format: `fine` in `ink` inside a square
 * `rule` outline, in the accent where the fact is a match. A tinted capsule in
 * the platform's look.
 */
@Composable
fun KoanBadge(text: String, modifier: Modifier = Modifier, accent: Boolean = false) {
    if (KoanTheme.isOn) {
        Box(
            modifier
                .border(KoanHairline, if (accent) KoanTheme.look.accent else KoanTheme.colors.rule)
                .padding(horizontal = 7.dp, vertical = 2.dp),
        ) {
            KoanText(text, role = KoanType.Fine, tone = if (accent) KoanTone.Accent else KoanTone.Ink, maxLines = 1)
        }
    } else {
        val scheme = MaterialTheme.colorScheme
        Text(
            text,
            modifier
                .background(if (accent) scheme.primary.copy(alpha = 0.15f) else scheme.surfaceVariant, CircleShape)
                .padding(horizontal = 8.dp, vertical = 3.dp),
            style = MaterialTheme.typography.labelSmall,
            color = if (accent) scheme.primary else scheme.onSurfaceVariant,
            maxLines = 1,
        )
    }
}

/**
 * A chip that is a choice (a device, a preset, an artist): a `muted`
 * outline, square, in the theme. A filled capsule in the platform's look.
 */
@Composable
fun Modifier.koanChip(): Modifier =
    if (KoanTheme.isOn) {
        border(KoanHairline, KoanTheme.colors.muted).padding(horizontal = 12.dp, vertical = 6.dp)
    } else {
        background(MaterialTheme.colorScheme.surfaceVariant, CircleShape).padding(horizontal = 12.dp, vertical = 6.dp)
    }

/** An empty page: its glyph in `muted`, what is missing in `body`, and why in `meta`. */
@Composable
fun KoanUnavailable(
    title: String,
    icon: Int,
    detail: String?,
    modifier: Modifier = Modifier,
    actions: (@Composable RowScope.() -> Unit)? = null,
) {
    Box(modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Column(
            Modifier.widthIn(max = 420.dp).padding(KoanSpace.xxl),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(KoanSpace.m),
        ) {
            KoanIcon(icon, contentDescription = null, size = 24.sp, color = KoanTheme.color(KoanTone.Muted))
            KoanText(title, appText = true, textAlign = TextAlign.Center)
            detail?.let { KoanText(it, role = KoanType.Meta, tone = KoanTone.Muted, textAlign = TextAlign.Center) }
            actions?.let { Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.m), content = it) }
        }
    }
}

/** Content in a role's type and a tone, for pieces that draw their own text and glyphs. */
@Composable
internal fun ProvideKoanType(role: KoanType?, color: Color, content: @Composable () -> Unit) {
    CompositionLocalProvider(
        LocalContentColor provides color,
        LocalTextStyle provides (role?.style ?: LocalTextStyle.current).copy(color = color),
        content = content,
    )
}
