package cc.blit.koan.theme

import kotlin.math.atan2
import kotlin.math.cos
import kotlin.math.sin

/**
 * The accent for a record, tone-mapped as `docs/design/koan-theme.md` sets
 * out: the sleeve's hue kept, its lightness and chroma moved into a band per
 * appearance in OKLCH. Mint when there is no record or no usable hue. The
 * port of `KoanAccent` in `KoanTheme.swift`.
 *
 * A value, with equality, so a collector of the accent is woken only by a
 * different one.
 */
data class KoanAccent(val dark: Shade, val light: Shade) {

    /**
     * An sRGB colour as 0xRRGGBB, and whether it reaches 4.5:1 on `bg` and
     * `surface`. Where it does not, it is used for fills, indicators and rings
     * only, and text stays ink.
     */
    data class Shade(val rgb: Int, val readsAsText: Boolean)

    fun shade(dark: Boolean): Shade = if (dark) this.dark else light

    companion object {
        val Mint = KoanAccent(Shade(0x7DD3A7, true), Shade(0x1F7A50, true))

        /** Lightness bands per appearance: never dark in dark mode, never pastel in light. */
        val DarkBand = 0.70..0.85
        val LightBand = 0.45..0.60

        /** Chroma floor and ceiling: a muddy sleeve gives a clean colour, a neon one does not flare. */
        val Chroma = 0.10..0.19

        /** Under this a sleeve has no hue worth carrying, and the accent is mint. */
        const val NoHue = 0.04

        /**
         * The accent for the colour a record reads as ([SleeveColour.dominant]);
         * null is no record, or none with a colour.
         */
        fun of(record: Int?): KoanAccent {
            if (record == null) return Mint
            val (_, c, h) = Oklch.fromSrgb(record)
            if (c < NoHue) return Mint
            val dark = shade(h, c, DarkBand, dark = true, bg = 0x1E1E1E, surface = 0x2A2A2A)
            val light = shade(h, c, LightBand, dark = false, bg = 0xFFFFFF, surface = 0xF2F2F2)
            return if (dark == null || light == null) Mint else KoanAccent(dark, light)
        }

        /**
         * The most vivid lightness in the band that reads as text (the darkest
         * in dark mode, the lightest in light) or failing that, the one that
         * clears 3:1 as a fill. Null if nothing in the band does.
         */
        fun shade(hue: Double, chroma: Double, band: ClosedFloatingPointRange<Double>, dark: Boolean, bg: Int, surface: Int): Shade? {
            val c = chroma.coerceIn(Chroma.start, Chroma.endInclusive)
            val steps = (0..50).map { band.start + (band.endInclusive - band.start) * it / 50.0 }
            val order = if (dark) steps else steps.reversed()
            val bgL = Oklch.luminance(bg)
            val surfaceL = Oklch.luminance(surface)
            for ((minimum, text) in listOf(4.5 to true, 3.0 to false)) {
                for (l in if (text) order else order.reversed()) {
                    val (r, g, b) = Oklch.toSrgb(l, c, hue)
                    val y = Oklch.luminance(r, g, b)
                    if (Oklch.contrast(y, bgL) >= minimum && Oklch.contrast(y, surfaceL) >= minimum) {
                        return Shade(pack(r, g, b), text)
                    }
                }
            }
            return null
        }

        private fun pack(r: Double, g: Double, b: Double): Int =
            (Math.round(r * 255).toInt() shl 16) or (Math.round(g * 255).toInt() shl 8) or Math.round(b * 255).toInt()
    }
}

/**
 * What colour a sleeve reads as: the port of `Color.dominant` in `Palette.swift`.
 *
 * Not the average of the sleeve, which gives the same brown-grey every time
 * because opposite hues cancel. A circular mean of hue weighted by each
 * sample's saturation, so the one strong colour on a mostly black cover wins
 * rather than being drowned by the black.
 */
object SleeveColour {
    /** The side of the thumbnail the analysis runs on. */
    const val SIDE = 32

    /**
     * From a thumbnail's pixels as ARGB, any number of them (the analysis is
     * per sample). Returns 0xRRGGBB, or null for a sleeve with no colour in it.
     */
    fun dominant(pixels: IntArray): Int? {
        var x = 0.0
        var y = 0.0
        var saturation = 0.0
        var total = 0.0
        for (argb in pixels) {
            val r = ((argb shr 16) and 0xFF) / 255.0
            val g = ((argb shr 8) and 0xFF) / 255.0
            val b = (argb and 0xFF) / 255.0
            val high = maxOf(r, g, b)
            val delta = high - minOf(r, g, b)
            val sat = if (high == 0.0) 0.0 else delta / high
            // Near-grey, near-black and blown-out samples say nothing about
            // what colour the record is.
            if (!(sat > 0.15 && high > 0.1 && high < 0.98)) continue
            val hue = hue(r, g, b, high, delta)
            val weight = sat * high
            val angle = hue * 2 * Math.PI
            x += cos(angle) * weight
            y += sin(angle) * weight
            saturation += sat * weight
            total += weight
        }
        if (total <= 0.0) return null
        val mean = atan2(y / total, x / total) / (2 * Math.PI)
        return hsvToRgb(if (mean < 0) mean + 1 else mean, (saturation / total).coerceIn(0.55, 0.85), 0.80)
    }

    private fun hue(r: Double, g: Double, b: Double, high: Double, delta: Double): Double {
        if (delta <= 0.0) return 0.0
        val h = when (high) {
            r -> ((g - b) / delta) % 6
            g -> (b - r) / delta + 2
            else -> (r - g) / delta + 4
        } / 6
        return if (h < 0) h + 1 else h
    }

    private fun hsvToRgb(h: Double, s: Double, v: Double): Int {
        val i = (h * 6).toInt() % 6
        val f = h * 6 - (h * 6).toInt()
        val p = v * (1 - s)
        val q = v * (1 - f * s)
        val t = v * (1 - (1 - f) * s)
        val (r, g, b) = when (i) {
            0 -> Triple(v, t, p)
            1 -> Triple(q, v, p)
            2 -> Triple(p, v, t)
            3 -> Triple(p, q, v)
            4 -> Triple(t, p, v)
            else -> Triple(v, p, q)
        }
        return (Math.round(r * 255).toInt() shl 16) or (Math.round(g * 255).toInt() shl 8) or Math.round(b * 255).toInt()
    }
}
