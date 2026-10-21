package cc.blit.koan.ui.library

import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalViewConfiguration
import androidx.compose.ui.platform.ViewConfiguration
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withLink
import androidx.compose.ui.unit.DpSize
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.ffi.CreditPart
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType

/**
 * One part of a line of detail: an artist or a record it opens the page of,
 * an artist credit whose credited artists each open theirs, or plain text.
 * Equal parts are equal, so a line whose parts have not moved is not built
 * again.
 */
@Immutable
data class Named(
    val text: String,
    val artist: Long? = null,
    val album: Long? = null,
    val track: Long? = null,
    val credit: List<CreditPart> = emptyList(),
) {
    val linkable get() = artist != null || album != null

    fun page() = if (artist != null) ArtistRoute(artist) else AlbumRoute(album!!, track)
}

/**
 * An artist credit as the engine built it, each credited artist a link but the
 * one whose page the line is on.
 */
fun artistCredit(credit: List<CreditPart>, except: Long? = null): Named {
    val parts = credit.map { if (it.artistId != null && it.artistId == except) it.copy(artistId = null) else it }
    return Named(parts.text, credit = parts)
}

/** A credit as plain text, for what cannot hold a link: the media session, a share. */
val List<CreditPart>.text get() = joinToString("") { it.text }

/** The artists a credit links, in the order it names them. */
fun creditedArtists(credit: List<CreditPart>) = credit.filter { it.artistId != null }

fun albumName(title: String, id: Long?, track: Long? = null) = Named(title, album = id, track = track)

/**
 * A line of detail under a title — "Artist · Album", with a time, a year or a
 * status beside them — each named artist and record a link to its page, the
 * rest plain. Links stay in the line's tone and turn `ink` while pressed; a
 * tap anywhere else on the line goes to the row. `linked = false` while
 * picking, so a tap on a name ticks the row instead.
 *
 * `dense` for a line under a title in a row or tile: its links are exactly
 * as large as their text. Compose otherwise extends a link to the 48-dp minimum
 * touch target, and the extension takes taps on the title above, the gap
 * between them and the row's padding, which open the row.
 */
@Composable
fun NameLine(
    parts: List<Named>,
    modifier: Modifier = Modifier,
    role: KoanType = KoanType.Meta,
    tone: KoanTone = KoanTone.Muted,
    linked: Boolean = true,
    maxLines: Int = 1,
    dense: Boolean = false,
) {
    val nav = LocalNavigator.current
    val pressed = KoanTheme.textColor(KoanTone.Ink)
    val text = remember(parts, linked, pressed, nav) {
        nameLineText(parts, linked, TextLinkStyles(pressedStyle = SpanStyle(color = pressed))) { nav.push(it) }
    }
    val config = LocalViewConfiguration.current
    val targets = remember(config, dense) {
        if (!dense) {
            config
        } else {
            object : ViewConfiguration by config {
                override val minimumTouchTargetSize get() = DpSize.Zero
            }
        }
    }
    CompositionLocalProvider(LocalViewConfiguration provides targets) {
        Text(text, modifier, style = role.style, color = KoanTheme.textColor(tone), maxLines = maxLines, overflow = TextOverflow.Ellipsis)
    }
}

/** A line's parts joined by dots, its links opening their pages through `open`. */
fun nameLineText(parts: List<Named>, linked: Boolean, styles: TextLinkStyles?, open: (NavKey) -> Unit): AnnotatedString =
    buildAnnotatedString {
        fun link(text: String, page: () -> NavKey) = withLink(LinkAnnotation.Clickable(text, styles) { open(page()) }) { append(text) }
        parts.filter { it.text.isNotEmpty() }.forEachIndexed { i, part ->
            if (i > 0) append(" · ")
            when {
                !linked -> append(part.text)
                part.credit.isNotEmpty() -> part.credit.forEach { p ->
                    val id = p.artistId
                    if (id != null) link(p.text) { ArtistRoute(id) } else append(p.text)
                }
                part.linkable -> link(part.text, part::page)
                else -> append(part.text)
            }
        }
    }
