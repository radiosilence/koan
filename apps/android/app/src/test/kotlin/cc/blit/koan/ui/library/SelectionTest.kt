package cc.blit.koan.ui.library

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SelectionTest {
    private val ash = Playable.OfArtist(1, "Ash")
    private val fern = Playable.OfArtist(2, "Fern")
    private val moth = Playable.OfArtist(3, "Moth")

    @Test fun outsideTheModeATapIsNotTaken() {
        val selection = Selection()
        assertFalse(selection.take(ash))
        assertTrue(selection.picked.isEmpty())
    }

    @Test fun picksKeepTheOrderTheyWereTicked() {
        val selection = Selection()
        selection.begin()
        listOf(moth, ash, fern).forEach { assertTrue(selection.take(it)) }
        assertEquals(listOf(moth, ash, fern), selection.picked.toList())
    }

    @Test fun aSecondTapUnticks() {
        val selection = Selection()
        selection.begin()
        selection.take(ash)
        selection.take(fern)
        selection.take(ash)
        assertEquals(listOf(fern), selection.picked.toList())
        assertFalse(selection.contains(ash))
        assertTrue(selection.contains(Playable.OfArtist(2, "Fern")))
    }

    @Test fun endingForgetsThePick() {
        val selection = Selection()
        selection.begin()
        selection.take(ash)
        selection.end()
        assertFalse(selection.active)
        assertTrue(selection.picked.isEmpty())
        assertFalse(selection.contains(ash))
    }
}
