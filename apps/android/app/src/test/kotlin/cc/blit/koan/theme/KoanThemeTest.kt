package cc.blit.koan.theme

import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.graphics.vector.PathParser
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class KoanThemeTest {
    private fun contrast(a: Int, b: Int) = Oklch.contrast(Oklch.luminance(a), Oklch.luminance(b))
    private fun rgb(color: androidx.compose.ui.graphics.Color) = color.toArgb() and 0xFFFFFF

    @Test
    fun `the tokens read as the document says they do`() {
        val dark = KoanColors.of(KoanAccent.Mint, dark = true)
        val light = KoanColors.of(KoanAccent.Mint, dark = false)
        // docs/design/koan-theme.md, Contrast, against bg.
        assertEquals(10.4, contrast(rgb(dark.ink), rgb(dark.bg)), 0.1)
        assertEquals(5.3, contrast(rgb(dark.muted), rgb(dark.bg)), 0.1)
        assertEquals(9.3, contrast(rgb(dark.accent), rgb(dark.bg)), 0.1)
        assertEquals(5.6, contrast(rgb(dark.bad), rgb(dark.bg)), 0.1)
        assertEquals(1.4, contrast(rgb(dark.rule), rgb(dark.bg)), 0.1)
        assertEquals(12.6, contrast(rgb(light.ink), rgb(light.bg)), 0.1)
        assertEquals(5.7, contrast(rgb(light.muted), rgb(light.bg)), 0.1)
        assertEquals(5.3, contrast(rgb(light.accent), rgb(light.bg)), 0.1)
        assertEquals(5.1, contrast(rgb(light.bad), rgb(light.bg)), 0.1)
        // Muted on surface: 4.6 dark, 5.1 light.
        assertEquals(4.6, contrast(rgb(dark.muted), rgb(dark.surface)), 0.1)
        assertEquals(5.1, contrast(rgb(light.muted), rgb(light.surface)), 0.1)
    }

    @Test
    fun `the colour tokens are the document's, hex for hex`() {
        val dark = KoanColors.of(KoanAccent.Mint, dark = true)
        assertEquals(listOf(0x1E1E1E, 0x2A2A2A, 0x383838, 0x4D4D4D, 0xCCCCCC, 0xFFFFFF, 0x919191, 0x7DD3A7, 0xEF6B73),
            listOf(dark.bg, dark.surface, dark.rule, dark.hover, dark.ink, dark.strong, dark.muted, dark.accent, dark.bad).map(::rgb))
        val light = KoanColors.of(KoanAccent.Mint, dark = false)
        assertEquals(listOf(0xFFFFFF, 0xF2F2F2, 0xE0E0E0, 0xC4C4C4, 0x333333, 0x111111, 0x666666, 0x1F7A50, 0xC43F3F),
            listOf(light.bg, light.surface, light.rule, light.hover, light.ink, light.strong, light.muted, light.accent, light.bad).map(::rgb))
    }

    @Test
    fun `no record and no hue are mint`() {
        assertEquals(KoanAccent.Mint, KoanAccent.of(null))
        assertEquals("a grey sleeve has no hue to carry", KoanAccent.Mint, KoanAccent.of(0x808080))
        assertEquals(KoanAccent.Mint, KoanAccent.of(0x000000))
        assertNotEquals(KoanAccent.Mint, KoanAccent.of(0xCC3333))
    }

    @Test
    fun `a record's accent keeps its hue and sits in the band for each appearance`() {
        for (record in listOf(0xCC3333, 0x3366CC, 0xCCAA22, 0x22AA66, 0x9933CC, 0xE0603A)) {
            val hue = Oklch.fromSrgb(record).h
            val accent = KoanAccent.of(record)
            assertNotEquals(KoanAccent.Mint, accent)
            for ((dark, band) in listOf(true to KoanAccent.DarkBand, false to KoanAccent.LightBand)) {
                val shade = accent.shade(dark)
                val lch = Oklch.fromSrgb(shade.rgb)
                assertTrue("lightness ${lch.l} of ${Integer.toHexString(record)} in $band", lch.l in (band.start - 0.01)..(band.endInclusive + 0.01))
                val drift = Math.abs(((lch.h - hue + 540) % 360) - 180)
                assertTrue("hue is never moved: $hue became ${lch.h}", drift < 8)
            }
        }
    }

    @Test
    fun `text in the accent reaches 4,5 to 1 on bg and on surface, or is not text`() {
        for (h in 0 until 360 step 10) {
            val (r, g, b) = Oklch.toSrgb(0.6, 0.15, h.toDouble())
            val record = (Math.round(r * 255).toInt() shl 16) or (Math.round(g * 255).toInt() shl 8) or Math.round(b * 255).toInt()
            val accent = KoanAccent.of(record)
            for ((dark, bg, surface) in listOf(Triple(true, 0x1E1E1E, 0x2A2A2A), Triple(false, 0xFFFFFF, 0xF2F2F2))) {
                val shade = accent.shade(dark)
                val minimum = if (shade.readsAsText) 4.5 else 3.0
                assertTrue("hue $h dark=$dark on bg", contrast(shade.rgb, bg) >= minimum - 0.05)
                assertTrue("hue $h dark=$dark on surface", contrast(shade.rgb, surface) >= minimum - 0.05)
            }
        }
    }

    @Test
    fun `where the accent cannot be read as text, text stays ink`() {
        val unreadable = KoanAccent(KoanAccent.Shade(0x555555, false), KoanAccent.Shade(0xAAAAAA, false))
        val colors = KoanColors.of(unreadable, dark = true)
        assertEquals(colors.ink, colors.tone(KoanTone.Accent))
        val mint = KoanColors.of(KoanAccent.Mint, dark = true)
        assertEquals(mint.accent, mint.tone(KoanTone.Accent))
    }

    @Test
    fun `the colour a sleeve reads as is its strong hue, not the grey average`() {
        assertNull(SleeveColour.dominant(IntArray(64) { 0xFF808080.toInt() }))
        assertNull("black says nothing", SleeveColour.dominant(IntArray(64) { 0xFF000000.toInt() }))
        // Mostly black with one strong blue: the blue wins rather than being drowned.
        val pixels = IntArray(1024) { if (it % 16 == 0) 0xFF2255DD.toInt() else 0xFF000000.toInt() }
        val colour = SleeveColour.dominant(pixels)!!
        val blue = Oklch.fromSrgb(0x2255DD).h
        val got = Oklch.fromSrgb(colour).h
        assertTrue("blue is $blue, got $got", Math.abs(blue - got) < 20)
        // Opposite hues do not cancel to brown-grey: the stronger one wins.
        val mixed = IntArray(100) { if (it < 70) 0xFFDD2222.toInt() else 0xFF22DDDD.toInt() }
        val red = Oklch.fromSrgb(0xDD2222).h
        assertTrue(Math.abs(Oklch.fromSrgb(SleeveColour.dominant(mixed)!!).h - red) < 25)
    }

    @Test
    fun `the glyph set is the apps' set and every outline parses`() {
        assertTrue(KoanGlyph.names.size >= 100)
        for (name in listOf("play", "pause", "next", "previous", "search", "heart", "heart-filled", "queue", "settings")) {
            // `queue` is allowed to be absent under that name; the others are the transport.
            if (name != "queue") assertTrue("$name is in the set", name in KoanGlyph.names)
        }
        var withFill = 0
        for (name in KoanGlyph.names) {
            val data = GlyphData.getValue(name)
            assertTrue("$name has an outline", data.strokes.isNotEmpty() || data.fills != null)
            if (data.strokes.isNotEmpty()) assertTrue(name, PathParser().parsePathString(data.strokes).toNodes().isNotEmpty())
            data.fills?.let {
                withFill++
                assertTrue(name, PathParser().parsePathString(it).toNodes().isNotEmpty())
            }
        }
        assertTrue("favourited and selecting have a fill layer", withFill >= 2)
    }

    @Test
    fun `glyph size and stroke follow the type beside them`() {
        assertEquals(18f, KoanGlyph.side(15f), 0f)
        assertEquals(1.2f, KoanGlyph.strokeWidth(15f), 0.001f)
        assertEquals(1.4f, KoanGlyph.strokeWidth(20f), 0.001f)
        assertEquals("never under a point", 1f, KoanGlyph.strokeWidth(4f), 0f)
    }

    @Test
    fun `the type roles are the document's`() {
        val expected = listOf(
            Triple(KoanRole.Display, 34f, 200), Triple(KoanRole.Title, 26f, 300), Triple(KoanRole.TitleSm, 22f, 300),
            Triple(KoanRole.Body, 15f, 400), Triple(KoanRole.Control, 14f, 400), Triple(KoanRole.Meta, 13f, 400),
            Triple(KoanRole.Fine, 12f, 400),
        )
        for ((role, size, weight) in expected) {
            assertEquals(size, role.size.value, 0f)
            assertEquals(weight, role.weight)
        }
        assertEquals(1.45, KoanRole.Body.lineHeight, 0.0)
    }

    @Test
    fun `colour tokens map onto a Material scheme as structure, as the document sets out`() {
        val c = KoanColors.of(KoanAccent.Mint, dark = true)
        val scheme = materialScheme(c)
        assertEquals(c.bg, scheme.background)
        assertEquals(c.surface, scheme.surfaceVariant)
        assertEquals(c.accent, scheme.primary)
        assertEquals(c.ink, scheme.onBackground)
        assertEquals(c.muted, scheme.onSurfaceVariant)
        assertEquals(c.rule, scheme.outlineVariant)
        assertEquals(c.bad, scheme.error)
        assertFalse(materialScheme(KoanColors.of(KoanAccent.Mint, dark = false)).background == scheme.background)
    }
}
