package cc.blit.koan.ui.theme

import androidx.annotation.DrawableRes
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.LocalTextStyle
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Fill
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.VectorGroup
import androidx.compose.ui.graphics.vector.VectorPath
import androidx.compose.ui.graphics.vector.toPath
import androidx.compose.ui.res.vectorResource
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.isSpecified
import kotlin.math.max
import kotlin.math.roundToInt

/**
 * One glyph of the kōan icon set (`KoanIcons.*`), drawn as the theme draws
 * icons: a single line in the colour of the text beside it, in a square 1.2
 * times the size of that text, with a stroke set by the size rather than
 * scaled with the drawing. In the platform's look too: Android has no symbol
 * set of its own that the glyphs stand in for.
 *
 * A glyph with a fill (favourited, selecting) has its strokes cut out of the
 * fill, or, given `fill`, the fill in that colour and the strokes over it.
 */
@Composable
fun KoanIcon(
    @DrawableRes icon: Int,
    contentDescription: String?,
    modifier: Modifier = Modifier,
    size: TextUnit = LocalTextStyle.current.fontSize,
    color: Color = LocalContentColor.current,
    fill: Color? = null,
) {
    val glyph = KoanGlyph.of(ImageVector.vectorResource(icon))
    val textSize = with(LocalDensity.current) { (if (size.isSpecified) size else KoanType.Body.koan.fontSize).toDp() }
    val side = KoanGlyph.side(textSize.value).dp
    val stroke = KoanGlyph.strokeWidth(textSize.value).dp
    val cutOut = glyph.fills != null && fill == null
    Canvas(
        modifier
            .size(side)
            .then(if (cutOut) Modifier.graphicsLayer(compositingStrategy = CompositingStrategy.Offscreen) else Modifier)
            .then(if (contentDescription != null) Modifier.semantics { this.contentDescription = contentDescription } else Modifier),
    ) {
        val unit = this.size.width / KoanGlyph.GRID
        val line = Stroke(width = stroke.toPx() / unit, cap = StrokeCap.Square, join = StrokeJoin.Miter)
        scale(unit, pivot = Offset.Zero) {
            glyph.fills?.let {
                drawPath(it, fill ?: color, style = Fill)
                drawPath(it, fill ?: color, style = line)
            }
            drawPath(glyph.strokes, color, style = line, blendMode = if (cutOut) BlendMode.Clear else BlendMode.SrcOver)
        }
    }
}

/** A glyph's two layers, read from its drawable's paths named `stroke` and `fill`. */
internal class KoanGlyph(val strokes: Path, val fills: Path?) {
    companion object {
        const val GRID = 24f

        /** The square a glyph is drawn in beside text of `size`. */
        fun side(size: Float) = (size * 1.2f).roundToInt().toFloat()

        /** The stroke beside text of `size`: 1.2 at 15, 1.4 at 20, never under 1. */
        fun strokeWidth(size: Float) = max(1f, 0.6f + 0.04f * size)

        @Composable
        fun of(vector: ImageVector): KoanGlyph = remember(vector) {
            val paths = vector.root.filterIsInstance<VectorPath>() +
                vector.root.filterIsInstance<VectorGroup>().flatMap { it.filterIsInstance<VectorPath>() }
            KoanGlyph(
                strokes = paths.firstOrNull { it.name == "stroke" }?.pathData?.toPath() ?: Path(),
                fills = paths.firstOrNull { it.name == "fill" }?.pathData?.toPath(),
            )
        }
    }
}
