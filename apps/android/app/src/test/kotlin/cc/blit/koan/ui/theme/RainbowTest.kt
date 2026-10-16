package cc.blit.koan.ui.theme

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** Gay mode's palette and its seven taps, and the graphics level's hold on the wash. */
class RainbowTest {
    @Test fun sevenQuickTapsSwitchIt() {
        val taps = SecretTaps()
        val results = (0 until 7).map { taps.tap(it * 300L) }
        assertEquals(List(6) { false } + true, results)
        assertFalse("the count starts again", taps.tap(2_100))
    }

    @Test fun aSlowTapStartsTheCountAgain() {
        val taps = SecretTaps()
        repeat(6) { taps.tap(it * 300L) }
        assertFalse(taps.tap(6 * 300L + 1_000))
        assertFalse(taps.tap(6 * 300L + 1_100))
    }

    @Test fun theAccentCyclesThroughTheHuesAndSkipsChrome() {
        val chrome = KoanAccent.of(Rainbow.hyperpop[2])
        val accents = (0 until 4).map(Rainbow::accent)
        assertEquals(4, accents.toSet().size)
        assertFalse(chrome in accents)
        assertEquals(Rainbow.accent(0), Rainbow.accent(4))
        assertEquals(Rainbow.accent(3), Rainbow.accent(-1))
    }

    @Test fun theGradientIsClosedAndTurns() {
        val stops = Rainbow.stops(1)
        assertEquals(Rainbow.hyperpop.size + 1, stops.size)
        assertEquals(stops.first(), stops.last())
        assertEquals(Rainbow.hyperpop[1], stops.first())
        assertNotEquals(Rainbow.stops(0), stops)
    }

    @Test fun graphicsLevelsReadFromEveryPlatform() {
        assertEquals(Graphics.Plain, Graphics.of(0))
        assertEquals(Graphics.Reduced, Graphics.of(1))
        assertEquals(Graphics.Full, Graphics.of(2))
        assertEquals(Graphics.Plain, Graphics.of(3))
        assertEquals(Graphics.Full, Graphics.of(42))
    }

    @Test fun graphicsHoldTheWash() {
        assertFalse(WashRules.shown(recordColours = true, graphics = Graphics.Plain))
        assertTrue(WashRules.shown(recordColours = true, graphics = Graphics.Reduced))
        assertFalse(WashRules.drifts(playing = true, visible = true, animations = true, powerSaving = false, graphics = Graphics.Reduced))
        assertTrue(WashRules.drifts(playing = true, visible = true, animations = true, powerSaving = false, graphics = Graphics.Full))
    }
}
