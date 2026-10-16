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
        val selection = playableSelection()
        assertFalse(selection.take(ash))
        assertTrue(selection.picked.isEmpty())
    }

    @Test fun picksKeepTheOrderTheyWereTicked() {
        val selection = playableSelection()
        selection.begin()
        listOf(moth, ash, fern).forEach { assertTrue(selection.take(it)) }
        assertEquals(listOf(moth, ash, fern), selection.picked.toList())
    }

    @Test fun aSecondTapUnticks() {
        val selection = playableSelection()
        selection.begin()
        selection.take(ash)
        selection.take(fern)
        selection.take(ash)
        assertEquals(listOf(fern), selection.picked.toList())
        assertFalse(selection.contains(ash))
        assertTrue(selection.contains(Playable.OfArtist(2, "Fern")))
    }

    @Test fun endingForgetsThePick() {
        val selection = playableSelection()
        selection.begin()
        selection.take(ash)
        selection.end()
        assertFalse(selection.active)
        assertTrue(selection.picked.isEmpty())
        assertFalse(selection.contains(ash))
    }

    @Test fun entriesOfTheSameTrackArePickedApartByTheirOwnKey() {
        data class Entry(val id: Long, val trackId: Long)
        val selection = Selection<Entry> { it.id }
        selection.begin()
        selection.take(Entry(1, 7))
        selection.take(Entry(2, 7))
        assertEquals(listOf(1L, 2L), selection.picked.map { it.id })
        selection.take(Entry(1, 7))
        assertEquals(listOf(2L), selection.picked.map { it.id })
        assertFalse(selection.contains(Entry(1, 7)))
    }
}
