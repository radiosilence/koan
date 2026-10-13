package cc.blit.koan.ui.theme

import android.graphics.Bitmap
import androidx.compose.runtime.Immutable
import androidx.compose.ui.graphics.Color
import androidx.core.graphics.scale
import kotlin.math.PI
import kotlin.math.atan2
import kotlin.math.cbrt
import kotlin.math.cos
import kotlin.math.max
import kotlin.math.min
import kotlin.math.pow
import kotlin.math.sin
import kotlin.math.sqrt

/**
 * The accent for a record, tone-mapped as the spec sets out: the sleeve's hue
 * kept, its lightness and chroma moved into a band per appearance in OKLCH.
 * Mint when there is no record or no usable hue. The Swift apps' `KoanAccent`,
 * value for value.
 */
@Immutable
data class KoanAccent(val dark: Shade, val light: Shade) {
    /**
     * One appearance's accent. `readsAsText` is whether it reaches 4.5:1 on
     * `bg` and `surface`; where it does not, it is used for fills, indicators
     * and rings only, and text stays ink.
     */
    @Immutable
    data class Shade(val color: Color, val readsAsText: Boolean)

    fun shade(dark: Boolean) = if (dark) this.dark else light

    companion object {
        val Mint = KoanAccent(
            dark = Shade(Color(0xFF7DD3A7), readsAsText = true),
            light = Shade(Color(0xFF1F7A50), readsAsText = true),
        )

        /** Lightness bands, per appearance: never dark in dark mode, never pastel in light. */
        private val darkBand = 0.70..0.85
        private val lightBand = 0.45..0.60

        /** Chroma floor and ceiling: a muddy sleeve gives a clean colour, a neon one does not flare. */
        private val chroma = 0.10..0.19

        /** Under this, a sleeve has no hue worth carrying, and the accent is mint. */
        private const val NO_HUE = 0.04

        private val cache = object : LinkedHashMap<ULong, KoanAccent>(64, 0.75f, true) {
            override fun removeEldestEntry(eldest: MutableMap.MutableEntry<ULong, KoanAccent>) = size > 64
        }

        /**
         * The accent for a record's colour, as `dominant` gives it; `null` is no
         * record, or none with a colour. Worked out once per colour, so the same
         * record always gives the same value.
         */
        fun of(record: Color?): KoanAccent {
            record ?: return Mint
            val key = record.value
            synchronized(cache) { cache[key]?.let { return it } }
            val made = make(record)
            synchronized(cache) { cache[key] = made }
            return made
        }

        /** The accent for a sleeve, decoded. */
        fun of(sleeve: Bitmap): KoanAccent = of(dominant(sleeve))

        private fun make(record: Color): KoanAccent {
            val (_, c, h) = Oklch.from(record.red.toDouble(), record.green.toDouble(), record.blue.toDouble())
            if (c < NO_HUE) return Mint
            val dark = shade(h, c, darkBand, darkMode = true, KoanColors.Dark) ?: return Mint
            val light = shade(h, c, lightBand, darkMode = false, KoanColors.Light) ?: return Mint
            return KoanAccent(dark, light)
        }

        /**
         * The most vivid lightness in the band that reads as text, the darkest
         * in dark mode and the lightest in light, or failing that the one that
         * clears 3:1 as a fill.
         */
        private fun shade(
            hue: Double,
            chroma: Double,
            band: ClosedFloatingPointRange<Double>,
            darkMode: Boolean,
            tokens: KoanColors,
        ): Shade? {
            val c = chroma.coerceIn(this.chroma)
            val steps = (0..50).map { band.start + (band.endInclusive - band.start) * it / 50 }
            val order = if (darkMode) steps else steps.reversed()
            val bg = Oklch.luminance(tokens.bg)
            val surface = Oklch.luminance(tokens.surface)
            fun contrast(a: Double, b: Double) = (max(a, b) + 0.05) / (min(a, b) + 0.05)
            for ((minimum, text) in listOf(4.5 to true, 3.0 to false)) {
                for (l in if (text) order else order.reversed()) {
                    val (r, g, b) = Oklch.toSrgb(l, c, hue)
                    val y = Oklch.luminance(r, g, b)
                    if (contrast(y, bg) >= minimum && contrast(y, surface) >= minimum) {
                        return Shade(Color(r.toFloat(), g.toFloat(), b.toFloat()), text)
                    }
                }
            }
            return null
        }

        /**
         * The colour a record reads as. Not the average of the sleeve, which
         * comes out the same brown-grey for every busy cover as opposite hues
         * cancel: a circular mean of hue weighted by each sample's chroma, so
         * the one strong colour on a mostly black cover wins. Samples are gated
         * on chroma rather than brightness, since thin strokes on black come out
         * of the downscale dark but still coloured. The Swift apps'
         * `Color.dominant`, so a record gives one accent on every device.
         */
        fun dominant(sleeve: Bitmap): Color? {
            val side = 32
            val small = sleeve.scale(side, side)
            val pixels = IntArray(side * side)
            small.getPixels(pixels, 0, side, 0, 0, side, side)
            if (small !== sleeve) small.recycle()

            var x = 0.0
            var y = 0.0
            var saturation = 0.0
            var total = 0.0
            for (pixel in pixels) {
                val (hue, sat, value) = hsb(
                    ((pixel shr 16) and 0xFF) / 255.0,
                    ((pixel shr 8) and 0xFF) / 255.0,
                    (pixel and 0xFF) / 255.0,
                )
                // Near-grey, near-black and blown-out samples say nothing about
                // what colour the record is.
                if (sat <= 0.15 || value <= 0.1 || value >= 0.98) continue
                val weight = sat * value
                val angle = hue * 2 * PI
                x += cos(angle) * weight
                y += sin(angle) * weight
                saturation += sat * weight
                total += weight
            }
            if (total <= 0) return null
            val mean = atan2(y / total, x / total) / (2 * PI)
            return Color.hsv(
                hue = ((if (mean < 0) mean + 1 else mean) * 360).toFloat(),
                saturation = (saturation / total).coerceIn(0.55, 0.85).toFloat(),
                value = 0.80f,
            )
        }

        private fun hsb(r: Double, g: Double, b: Double): Triple<Double, Double, Double> {
            val high = maxOf(r, g, b)
            val low = minOf(r, g, b)
            val delta = high - low
            if (delta <= 0) return Triple(0.0, 0.0, high)
            var hue = when (high) {
                r -> ((g - b) / delta).rem(6)
                g -> (b - r) / delta + 2
                else -> (r - g) / delta + 4
            }
            hue /= 6
            return Triple(if (hue < 0) hue + 1 else hue, if (high == 0.0) 0.0 else delta / high, high)
        }
    }
}

/**
 * OKLab in polar form: what the accent is tone-mapped in, because its
 * lightness is perceptual and its hue holds still while lightness moves.
 */
internal object Oklch {
    /** Lightness, chroma and hue in degrees, from gamma-encoded sRGB in 0..1. */
    fun from(r: Double, g: Double, b: Double): Triple<Double, Double, Double> {
        val lr = linear(r)
        val lg = linear(g)
        val lb = linear(b)
        val l = cbrt(0.4122214708 * lr + 0.5363325363 * lg + 0.0514459929 * lb)
        val m = cbrt(0.2119034982 * lr + 0.6806995451 * lg + 0.1073969566 * lb)
        val s = cbrt(0.0883024619 * lr + 0.2817188376 * lg + 0.6299787005 * lb)
        val lightness = 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s
        val a = 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s
        val bb = 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s
        val hue = (atan2(bb, a) * 180 / PI + 360).rem(360)
        return Triple(lightness, sqrt(a * a + bb * bb), hue)
    }

    /** sRGB, with chroma given up until the colour is inside the gamut. */
    fun toSrgb(l: Double, c: Double, h: Double): Triple<Double, Double, Double> {
        var chroma = c
        while (true) {
            val (r, g, b) = toLinear(l, chroma, h)
            val inside = listOf(r, g, b).all { it >= -0.0001 && it <= 1.0001 }
            if (inside || chroma <= 0) return Triple(gamma(r), gamma(g), gamma(b))
            chroma -= 0.002
        }
    }

    fun luminance(color: Color) = luminance(color.red.toDouble(), color.green.toDouble(), color.blue.toDouble())

    fun luminance(r: Double, g: Double, b: Double) = 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)

    private fun toLinear(l: Double, c: Double, h: Double): Triple<Double, Double, Double> {
        val a = c * cos(h * PI / 180)
        val b = c * sin(h * PI / 180)
        val l3 = (l + 0.3963377774 * a + 0.2158037573 * b).pow(3)
        val m3 = (l - 0.1055613458 * a - 0.0638541728 * b).pow(3)
        val s3 = (l - 0.0894841775 * a - 1.2914855480 * b).pow(3)
        return Triple(
            4.0767416621 * l3 - 3.3077115913 * m3 + 0.2309699292 * s3,
            -1.2684380046 * l3 + 2.6097574011 * m3 - 0.3413193965 * s3,
            -0.0041960863 * l3 - 0.7034186147 * m3 + 1.7076147010 * s3,
        )
    }

    private fun linear(v: Double) = if (v <= 0.04045) v / 12.92 else ((v + 0.055) / 1.055).pow(2.4)

    private fun gamma(v: Double): Double {
        val c = v.coerceIn(0.0, 1.0)
        return if (c <= 0.0031308) 12.92 * c else 1.055 * c.pow(1 / 2.4) - 0.055
    }
}
