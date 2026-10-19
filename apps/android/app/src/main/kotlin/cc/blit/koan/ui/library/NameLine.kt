package cc.blit.koan.ui.library

import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withLink
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType

/**
 * One part of a line of detail: an artist or a record it opens the page of,
 * or plain text. Equal parts are equal, so a line whose parts have not moved
 * is not built again.
 */
@Immutable
data class Named(val text: String, val artist: Long? = null, val album: Long? = null, val track: Long? = null) {
    val linkable get() = artist != null || album != null

    fun page() = if (artist != null) ArtistRoute(artist) else AlbumRoute(album!!, track)
}

fun artistName(name: String, id: Long?) = Named(name, artist = id)

fun albumName(title: String, id: Long?, track: Long? = null) = Named(title, album = id, track = track)

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
    val pressed = KoanTheme.textColor(KoanTone.Ink)
    val text = remember(parts, linked, pressed, nav) {
        val styles = TextLinkStyles(pressedStyle = SpanStyle(color = pressed))
        buildAnnotatedString {
            parts.filter { it.text.isNotEmpty() }.forEachIndexed { i, part ->
                if (i > 0) append(" · ")
                if (linked && part.linkable) {
                    withLink(LinkAnnotation.Clickable(part.text, styles) { nav.push(part.page()) }) { append(part.text) }
                } else {
                    append(part.text)
                }
            }
        }
    }
    Text(text, modifier, style = role.style, color = KoanTheme.textColor(tone), maxLines = maxLines, overflow = TextOverflow.Ellipsis)
}
