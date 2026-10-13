package cc.blit.koan.ui.theme

import kotlin.math.pow

/**
 * The theme's hold on the wash: every sample of the baked sleeve kept on the
 * side of a luminance limit where `muted` text over it still reads. No
 * brighter in dark mode, no darker in light. The hue survives; the brightness
 * that would cost contrast does not. The Apple apps' `WashTone`.
 */
enum class WashTone(internal val limit: Double) {
    /** No brighter than `muted` (`#919191`) at 4.6:1 allows. */
    Dark((0.2831 + 0.05) / 4.6 - 0.05),

    /** No darker than keeps the wash, drawn over white, at least as light as `surface`. */
    Light(0.86),
}

/**
 * When the wash is drawn and when it moves. Shown wherever colours come from
 * the record; it breathes only while something plays, the app is in view,
 * the system's animations are on and it is not saving power. Otherwise it
 * settles where it stands.
 */
internal object WashRules {
    fun shown(recordColours: Boolean) = recordColours

    fun drifts(playing: Boolean, visible: Boolean, animations: Boolean, powerSaving: Boolean) =
        playing && visible && animations && !powerSaving
}

/**
 * The sleeve made into the wash, once per record and appearance: saturated,
 * blurred to colour fields, held to the tone's limit, and laid over the
 * ground at the wash's strength, so the result is opaque and the dissolve
 * between two records is one fade. Everything after this is the compositor
 * moving a small bitmap.
 *
 * Pure arithmetic on `0xAARRGGBB` pixels, so it runs, and is tested, off any
 * device.
 */
internal object WashBake {
    /**
     * The texture's side. Blurred to mush, so drawn small and magnified: the
     * Apple apps blur 14 points of a 360-point texture, which is 5 pixels of
     * this one, and bilinear magnification of something that soft adds nothing
     * the eye can find.
     */
    const val SIDE = 128
    private const val RADIUS = 5
    private const val SATURATION = 1.6

    fun bake(pixels: IntArray, side: Int, tone: WashTone, ground: Int, strength: Float): IntArray {
        require(pixels.size == side * side)
        val n = pixels.size
        val r = DoubleArray(n)
        val g = DoubleArray(n)
        val b = DoubleArray(n)
        for (i in 0 until n) {
            val p = pixels[i]
            val lr = DECODE[(p shr 16) and 0xFF]
            val lg = DECODE[(p shr 8) and 0xFF]
            val lb = DECODE[p and 0xFF]
            // Core Image's colour controls: away from luma by the saturation.
            val y = 0.2125 * lr + 0.7154 * lg + 0.0721 * lb
            r[i] = (y + (lr - y) * SATURATION).coerceIn(0.0, 1.0)
            g[i] = (y + (lg - y) * SATURATION).coerceIn(0.0, 1.0)
            b[i] = (y + (lb - y) * SATURATION).coerceIn(0.0, 1.0)
        }
        for (channel in arrayOf(r, g, b)) blur(channel, side)

        val gr = (ground shr 16) and 0xFF
        val gg = (ground shr 8) and 0xFF
        val gb = ground and 0xFF
        val out = IntArray(n)
        for (i in 0 until n) {
            var lr = r[i]
            var lg = g[i]
            var lb = b[i]
            val y = 0.2126 * lr + 0.7152 * lg + 0.0722 * lb
            when {
                tone == WashTone.Dark && y > tone.limit -> {
                    val k = tone.limit / y
                    lr *= k
                    lg *= k
                    lb *= k
                }
                tone == WashTone.Light && y < tone.limit -> {
                    val t = (tone.limit - y) / (1 - y)
                    lr += (1 - lr) * t
                    lg += (1 - lg) * t
                    lb += (1 - lb) * t
                }
            }
            // Over the ground in encoded values, as a layer's opacity composites.
            out[i] = (0xFF shl 24) or
                (mix(gr, encode(lr), strength) shl 16) or
                (mix(gg, encode(lg), strength) shl 8) or
                mix(gb, encode(lb), strength)
        }
        return out
    }

    /**
     * Three box passes, about a Gaussian of the radius, with the edge clamped:
     * an unclamped blur reads past the edge as black and gives the sleeve a
     * dark border.
     */
    private fun blur(channel: DoubleArray, side: Int) {
        val line = DoubleArray(side)
        repeat(3) {
            for (y in 0 until side) {
                for (x in 0 until side) line[x] = channel[y * side + x]
                boxed(line) { x, v -> channel[y * side + x] = v }
            }
            for (x in 0 until side) {
                for (y in 0 until side) line[y] = channel[y * side + x]
                boxed(line) { y, v -> channel[y * side + x] = v }
            }
        }
    }

    private inline fun boxed(line: DoubleArray, set: (Int, Double) -> Unit) {
        val last = line.size - 1
        val width = 2 * RADIUS + 1
        var sum = 0.0
        for (k in -RADIUS..RADIUS) sum += line[k.coerceIn(0, last)]
        for (i in line.indices) {
            set(i, sum / width)
            sum += line[(i + RADIUS + 1).coerceAtMost(last)] - line[(i - RADIUS).coerceAtLeast(0)]
        }
    }

    private fun mix(ground: Int, wash: Int, strength: Float) =
        (ground + (wash - ground) * strength + 0.5f).toInt().coerceIn(0, 255)

    private val DECODE = DoubleArray(256) {
        val c = it / 255.0
        if (c <= 0.04045) c / 12.92 else ((c + 0.055) / 1.055).pow(2.4)
    }

    private fun encode(v: Double): Int {
        val c = v.coerceIn(0.0, 1.0)
        val g = if (c <= 0.0031308) 12.92 * c else 1.055 * c.pow(1 / 2.4) - 0.055
        return (g * 255 + 0.5).toInt()
    }
}
