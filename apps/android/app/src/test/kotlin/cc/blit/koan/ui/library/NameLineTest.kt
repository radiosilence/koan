package cc.blit.koan.ui.library

import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.ffi.CreditPart
import org.junit.Assert.assertEquals
import org.junit.Test

/** A line of detail's links: each credited artist its own, the joining text plain. */
class NameLineTest {
    private val credit = listOf(
        CreditPart("Orchestre Tout Puissant Marcel Duchamp", 1),
        CreditPart(" × ", null),
        CreditPart("Frànçois & the Atlas Mountains", 2),
    )

    /** Each link's text, and the page a tap on it opens. */
    private fun links(parts: List<Named>, linked: Boolean = true): Pair<String, List<Pair<String, NavKey>>> {
        var opened: NavKey? = null
        val text: AnnotatedString = nameLineText(parts, linked, null) { opened = it }
        return text.text to text.getLinkAnnotations(0, text.length).map { range ->
            val link = range.item as LinkAnnotation.Clickable
            link.linkInteractionListener!!.onClick(link)
            text.substring(range.start, range.end) to opened!!
        }
    }

    private fun NavKey.id() = when (this) {
        is ArtistRoute -> "artist ${artistId}"
        is AlbumRoute -> "album ${albumId}"
        else -> error("unexpected $this")
    }

    @Test
    fun eachCreditedArtistIsItsOwnLink() {
        val (text, links) = links(listOf(artistCredit(credit), albumName("Ventre Unique", 5)))
        assertEquals("Orchestre Tout Puissant Marcel Duchamp × Frànçois & the Atlas Mountains · Ventre Unique", text)
        assertEquals(
            listOf(
                "Orchestre Tout Puissant Marcel Duchamp" to "artist 1",
                "Frànçois & the Atlas Mountains" to "artist 2",
                "Ventre Unique" to "album 5",
            ),
            links.map { (t, page) -> t to page.id() },
        )
    }

    @Test
    fun theArtistWhosePageItIsStaysText() {
        val (text, links) = links(listOf(artistCredit(credit, except = 2)))
        assertEquals("Orchestre Tout Puissant Marcel Duchamp × Frànçois & the Atlas Mountains", text)
        assertEquals(listOf("Orchestre Tout Puissant Marcel Duchamp" to "artist 1"), links.map { (t, page) -> t to page.id() })
    }

    @Test
    fun anUnsplitCreditIsTheNameAndItsArtist() {
        val (text, links) = links(listOf(artistCredit(listOf(CreditPart("Burial", 7)))))
        assertEquals("Burial", text)
        assertEquals(listOf("Burial" to "artist 7"), links.map { (t, page) -> t to page.id() })
    }

    @Test
    fun whilePickingNothingIsALink() {
        val (text, links) = links(listOf(artistCredit(credit)), linked = false)
        assertEquals("Orchestre Tout Puissant Marcel Duchamp × Frànçois & the Atlas Mountains", text)
        assertEquals(emptyList<Pair<String, NavKey>>(), links)
    }

    @Test
    fun theMenuOffersEachCreditedArtist() {
        assertEquals(listOf(1L, 2L), creditedArtists(credit).map { it.artistId })
    }
}
