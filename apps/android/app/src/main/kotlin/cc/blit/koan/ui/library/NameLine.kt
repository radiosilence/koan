package cc.blit.koan.ui.library

import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withLink
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType

/** One part of a line of detail, and the page it opens, if it names something with one. */
class Named(val text: String, val page: (() -> NavKey)? = null)

fun artistName(name: String, id: Long?) = Named(name, id?.let { { ArtistRoute(it) } })

fun albumName(title: String, id: Long?, track: Long? = null) = Named(title, id?.let { { AlbumRoute(it, track) } })

/**
 * A line of detail under a title — "Artist · Album", with a time, a year or a
 * status beside them — each named artist and record a link to its page, the
 * rest plain. Links stay in the line's tone and turn `ink` while pressed; a
 * tap anywhere else on the line goes to the row. `linked = false` while
 * picking, so a tap on a name ticks the row instead.
 */
@Composable
fun NameLine(
    parts: List<Named>,
    modifier: Modifier = Modifier,
    role: KoanType = KoanType.Meta,
    tone: KoanTone = KoanTone.Muted,
    linked: Boolean = true,
    maxLines: Int = 1,
) {
    val nav = LocalNavigator.current
    val styles = TextLinkStyles(pressedStyle = SpanStyle(color = KoanTheme.textColor(KoanTone.Ink)))
    val shown = parts.filter { it.text.isNotEmpty() }
    val text = buildAnnotatedString {
        shown.forEachIndexed { i, part ->
            if (i > 0) append(" · ")
            val page = part.page
            if (linked && page != null) {
                withLink(LinkAnnotation.Clickable(part.text, styles) { nav.push(page()) }) { append(part.text) }
            } else {
                append(part.text)
            }
        }
    }
    Text(text, modifier, style = role.style, color = KoanTheme.textColor(tone), maxLines = maxLines, overflow = TextOverflow.Ellipsis)
}
