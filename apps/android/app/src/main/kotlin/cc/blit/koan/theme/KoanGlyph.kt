package cc.blit.koan.theme

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Fill
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/** The outlines of one glyph as path data; see the generated `GlyphData`. */
internal class GlyphPaths(val strokes: String, val fills: String? = null)

/**
 * One glyph of the kōan icon set: outlines on a 24-unit grid, compiled from
 * `apps/macos/Resources/Icons` by `just icons` into `KoanGlyphs.kt`, the same
 * sources the Apple apps draw. Strokes are one width at any size, set by
 * [strokeWidth] rather than scaled with the drawing, with square caps and
 * mitred joins. A glyph may have a fill layer (favourited, selecting); its
 * strokes are then cut out of the fill.
 */
class KoanGlyph internal constructor(val name: String, internal val strokes: Path, internal val fills: Path?) {
    companion object {
        const val GRID = 24f

        private val cache = HashMap<String, KoanGlyph?>()

        /** The glyph by file name (`play`, `heart-filled`), or null if the set has none. */
        fun named(name: String): KoanGlyph? = synchronized(cache) {
            cache.getOrPut(name) {
                GlyphData[name]?.let { data ->
                    KoanGlyph(name, parse(data.strokes), data.fills?.let(::parse))
                }
            }
        }

        val names: Set<String> get() = GlyphData.keys

        private fun parse(data: String): Path = PathParser().parsePathString(data).toPath()

        /** The square a glyph is drawn in beside type of `sizeDp`: the optical size of a symbol at that size. */
        fun side(sizeDp: Float): Float = Math.round(sizeDp * 1.2f).toFloat()

        /** The stroke beside type of `sizeDp`: 1.2 at 15, 1.4 at 20, never under one. */
        fun strokeWidth(sizeDp: Float): Float = maxOf(1f, 0.6f + 0.04f * sizeDp)
    }
}

/**
 * A glyph drawn in the theme: in the colour of the text beside it, at the
 * square [KoanGlyph.side] gives for [textSizeDp]. Decorative unless given a
 * [description].
 */
@Composable
fun KoanIcon(
    name: String,
    modifier: Modifier = Modifier,
    tone: KoanTone = KoanTone.Ink,
    textSizeDp: Float = 15f,
    description: String? = null,
) {
    val glyph = remember(name) { KoanGlyph.named(name) } ?: return
    val colour = LocalKoan.current.tone(tone)
    KoanIcon(glyph, colour, modifier, textSizeDp, description)
}

@Composable
fun KoanIcon(
    glyph: KoanGlyph,
    colour: Color,
    modifier: Modifier = Modifier,
    textSizeDp: Float = 15f,
    description: String? = null,
) {
    val side: Dp = KoanGlyph.side(textSizeDp).dp
    val stroke = KoanGlyph.strokeWidth(textSizeDp)
    var m = modifier.size(side)
    if (glyph.fills != null) m = m.graphicsLayer { compositingStrategy = CompositingStrategy.Offscreen }
    if (description != null) m = m.semantics { contentDescription = description }
    Canvas(m) {
        val scale = size.width / KoanGlyph.GRID
        withTransform({ scale(scale, scale, pivot = Offset.Zero) }) {
            val width = stroke * density / scale
            val style = Stroke(width = width, cap = StrokeCap.Square, join = StrokeJoin.Miter)
            glyph.fills?.let {
                drawPath(it, colour, style = Fill)
                drawPath(it, colour, style = style)
            }
            // Where there is a fill the strokes are cut out of it; without,
            // they are simply drawn.
            if (glyph.fills != null) {
                drawPath(glyph.strokes, Color.Black, style = style, blendMode = BlendMode.Clear)
            } else {
                drawPath(glyph.strokes, colour, style = style)
            }
        }
    }
}
