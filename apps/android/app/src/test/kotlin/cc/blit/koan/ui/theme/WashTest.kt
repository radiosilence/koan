package cc.blit.koan.ui.theme

import androidx.compose.ui.graphics.Color
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.math.abs
import kotlin.math.max
import kotlin.math.min

class WashTest {
    private val side = WashBake.SIDE
    private val darkBg = 0xFF1E1E1E.toInt()
    private val lightBg = 0xFFFFFFFF.toInt()

    /** Sleeves the limits are hardest on: pure primaries, white, black, and a split cover. */
    private val sleeves = listOf(
        0xFFFF0000, 0xFF00FF00, 0xFF0000FF, 0xFFFFFF00, 0xFFFFFFFF, 0xFF000000, 0xFF39FF14, 0xFF1B2A5C,
    ).map { solid(it.toInt()) } + listOf(split(0xFFFFFFFF.toInt(), 0xFFFF00FF.toInt()))

    @Test
    fun mutedTextReadsOverEveryDarkWash() {
        for (sleeve in sleeves) {
            val baked = WashBake.bake(sleeve, side, WashTone.Dark, darkBg, RecordWash.strength(koan = true))
            val worst = baked.minOf { contrast(it, 0xFF919191.toInt()) }
            assertTrue("muted at $worst:1", worst >= 4.5)
        }
    }

    @Test
    fun mutedTextReadsOverEveryLightWash() {
        for (sleeve in sleeves) {
            val baked = WashBake.bake(sleeve, side, WashTone.Light, lightBg, RecordWash.strength(koan = true))
            val worst = baked.minOf { contrast(it, 0xFF666666.toInt()) }
            assertTrue("muted at $worst:1", worst >= 4.5)
        }
    }

    @Test
    fun theThemeTintBlendsFromTheSleeveToTheHold() {
        // A bright sleeve, well above the dark limit, drawn at full strength so the ground plays no part.
        val sleeve = solid(0xFF39FF14.toInt())
        fun centre(hold: Double) = channels(WashBake.bake(sleeve, side, WashTone.Dark, darkBg, 1f, hold)[side * side / 2]).second
        val free = centre(0.0)
        val half = centre(0.5)
        val held = centre(1.0)
        assertEquals(channels(WashBake.bake(sleeve, side, WashTone.Dark, darkBg, 1f)[side * side / 2]).second, held)
        assertTrue("free $free > half $half > held $held", free > half && half > held)
    }

    @Test
    fun theWashKeepsTheSleevesHue() {
        val red = WashBake.bake(solid(0xFFC0302A.toInt()), side, WashTone.Dark, darkBg, 0.6f)
        val blue = WashBake.bake(solid(0xFF2A50C0.toInt()), side, WashTone.Dark, darkBg, 0.6f)
        val (r, _, b) = channels(red[side * side / 2])
        val (r2, _, b2) = channels(blue[side * side / 2])
        assertTrue(r > b)
        assertTrue(b2 > r2)
    }

    @Test
    fun theWashIsOpaqueSoTheDissolveIsOneFade() {
        val baked = WashBake.bake(split(0xFFFF0000.toInt(), 0x00000000), side, WashTone.Dark, darkBg, 0.6f)
        assertTrue(baked.all { it ushr 24 == 0xFF })
    }

    @Test
    fun theBlurLeavesNoDarkBorder() {
        // An unclamped blur reads past the edge as black.
        val baked = WashBake.bake(solid(0xFF808080.toInt()), side, WashTone.Dark, darkBg, 1f)
        val centre = channels(baked[side * side / 2 + side / 2])
        val corner = channels(baked[0])
        assertEquals(centre.first, corner.first)
    }

    @Test
    fun theBlurSoftensAnEdge() {
        val baked = WashBake.bake(split(0xFF000000.toInt(), 0xFFFFFFFF.toInt()), side, WashTone.Light, lightBg, 1f)
        val row = side / 2 * side
        val across = (0 until side).map { channels(baked[row + it]).first }
        // A ramp, not a step: many levels between the two sides, none of them a jump across half.
        assertTrue(across.toSet().size >= 8)
        assertTrue(across.zipWithNext().all { (a, b) -> abs(a - b) < (across.max() - across.min()) / 2 })
    }

    @Test
    fun theWashFollowsTheRecordColoursSetting() {
        assertTrue(WashRules.shown(recordColours = true))
        assertFalse(WashRules.shown(recordColours = false))
    }

    @Test
    fun theWashBreathesOnlyWhilePlayingInViewWithAnimationsOn() {
        assertTrue(WashRules.drifts(playing = true, visible = true, animations = true, powerSaving = false))
        assertFalse(WashRules.drifts(playing = false, visible = true, animations = true, powerSaving = false))
        assertFalse(WashRules.drifts(playing = true, visible = false, animations = true, powerSaving = false))
        assertFalse(WashRules.drifts(playing = true, visible = true, animations = false, powerSaving = false))
        assertFalse(WashRules.drifts(playing = true, visible = true, animations = true, powerSaving = true))
    }

    @Test
    fun theAccentKeepsTheRecordsHueAndReadsAsText() {
        val accent = KoanAccent.of(Color(0xFFB0302A))
        assertTrue(accent != KoanAccent.Mint)
        val (r, g, b) = accent.dark.color.let { Triple(it.red, it.green, it.blue) }
        assertTrue(r > g && r > b)
        for ((shade, bg) in listOf(accent.dark to darkBg, accent.light to lightBg)) {
            if (shade.readsAsText) assertTrue(contrast(shade.color.toArgbInt(), bg) >= 4.5)
        }
    }

    @Test
    fun aGreySleeveGivesMint() {
        assertEquals(KoanAccent.Mint, KoanAccent.of(Color(0xFF808080)))
        assertEquals(KoanAccent.Mint, KoanAccent.of(null))
    }

    @Test
    fun oneColourGivesOneAccent() {
        assertTrue(KoanAccent.of(Color(0xFF1B2A5C)) === KoanAccent.of(Color(0xFF1B2A5C)))
    }

    private fun solid(argb: Int) = IntArray(side * side) { argb }

    private fun split(left: Int, right: Int) = IntArray(side * side) { if (it % side < side / 2) left else right }

    private fun channels(argb: Int) = Triple((argb shr 16) and 0xFF, (argb shr 8) and 0xFF, argb and 0xFF)

    private fun Color.toArgbInt() =
        (0xFF shl 24) or ((red * 255 + 0.5f).toInt() shl 16) or ((green * 255 + 0.5f).toInt() shl 8) or (blue * 255 + 0.5f).toInt()

    private fun luminance(argb: Int): Double {
        fun lin(c: Int) = (c / 255.0).let { if (it <= 0.04045) it / 12.92 else Math.pow((it + 0.055) / 1.055, 2.4) }
        val (r, g, b) = channels(argb)
        return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
    }

    private fun contrast(a: Int, b: Int): Double {
        val x = luminance(a)
        val y = luminance(b)
        return (max(x, y) + 0.05) / (min(x, y) + 0.05)
    }
}
