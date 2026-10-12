package cc.blit.koan.theme

import kotlin.math.atan2
import kotlin.math.cbrt
import kotlin.math.cos
import kotlin.math.pow
import kotlin.math.sin
import kotlin.math.sqrt

/**
 * OKLab in polar form: what the accent is tone-mapped in, because its
 * lightness is perceptual and its hue holds still while lightness moves.
 * The port of `OKLCH` in `KoanTheme.swift`, number for number.
 */
object Oklch {
    data class Lch(val l: Double, val c: Double, val h: Double)

    /** From an sRGB colour as 0xRRGGBB. */
    fun fromSrgb(rgb: Int): Lch {
        val r = ((rgb shr 16) and 0xFF) / 255.0
        val g = ((rgb shr 8) and 0xFF) / 255.0
        val b = (rgb and 0xFF) / 255.0
        return fromLinear(linear(r), linear(g), linear(b))
    }

    fun fromLinear(r: Double, g: Double, b: Double): Lch {
        val l = cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b)
        val m = cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b)
        val s = cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b)
        val lightness = 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s
        val a = 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s
        val bb = 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s
        val hue = (Math.toDegrees(atan2(bb, a)) + 360.0) % 360.0
        return Lch(lightness, sqrt(a * a + bb * bb), hue)
    }

    /** sRGB components in 0…1, with chroma given up until the colour is inside the gamut. */
    fun toSrgb(l: Double, c: Double, h: Double): Triple<Double, Double, Double> {
        var chroma = c
        while (true) {
            val (lr, lg, lb) = toLinear(l, chroma, h)
            val inside = listOf(lr, lg, lb).all { it >= -0.0001 && it <= 1.0001 }
            if (inside || chroma <= 0) return Triple(gamma(lr), gamma(lg), gamma(lb))
            chroma -= 0.002
        }
    }

    /** Relative luminance (WCAG) of 0xRRGGBB. */
    fun luminance(rgb: Int): Double = luminance(
        ((rgb shr 16) and 0xFF) / 255.0, ((rgb shr 8) and 0xFF) / 255.0, (rgb and 0xFF) / 255.0,
    )

    fun luminance(r: Double, g: Double, b: Double): Double =
        0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)

    /** WCAG contrast ratio of two luminances. */
    fun contrast(a: Double, b: Double): Double = (maxOf(a, b) + 0.05) / (minOf(a, b) + 0.05)

    private fun toLinear(l: Double, c: Double, h: Double): Triple<Double, Double, Double> {
        val a = c * cos(Math.toRadians(h))
        val b = c * sin(Math.toRadians(h))
        val l3 = (l + 0.3963377774 * a + 0.2158037573 * b).pow(3)
        val m3 = (l - 0.1055613458 * a - 0.0638541728 * b).pow(3)
        val s3 = (l - 0.0894841775 * a - 1.2914855480 * b).pow(3)
        return Triple(
            4.0767416621 * l3 - 3.3077115913 * m3 + 0.2309699292 * s3,
            -1.2684380046 * l3 + 2.6097574011 * m3 - 0.3413193965 * s3,
            -0.0041960863 * l3 - 0.7034186147 * m3 + 1.7076147010 * s3,
        )
    }

    private fun linear(v: Double): Double = if (v <= 0.04045) v / 12.92 else ((v + 0.055) / 1.055).pow(2.4)

    private fun gamma(v: Double): Double {
        val x = v.coerceIn(0.0, 1.0)
        return if (x <= 0.0031308) 12.92 * x else 1.055 * x.pow(1 / 2.4) - 0.055
    }
}
