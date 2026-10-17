package cc.blit.koan.ui.theme

import android.animation.ValueAnimator
import android.graphics.Bitmap
import android.os.PowerManager
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.tween
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.graphics.FilterQuality
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntSize
import androidx.core.graphics.scale
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.currentStateAsState
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlin.math.max
import kotlin.math.roundToInt

/**
 * The record wash: the playing record's sleeve, blurred to colour fields and
 * drifting, behind the bare ground of a page. The one thing under the ground
 * that is not flat, and it carries data. The Apple apps' `DriftingWash`.
 *
 * Held once, at the root, so every page that draws it shows the same record
 * at the same point in its drift, and a sleeve is baked once however many
 * pages show it. Pages draw it with `Modifier.recordWash()`.
 *
 * Nothing here runs per frame but the three drift values, and only while
 * something plays. The texture is baked once per record (`WashBake`): a small
 * opaque bitmap the GPU magnifies, never a live blur, which the compositor
 * would re-run on every frame of the drift.
 */
@Stable
class RecordWash internal constructor() {
    /** The record shown, and the one it is dissolving from. */
    internal var current by mutableStateOf<ImageBitmap?>(null)
    internal var previous by mutableStateOf<ImageBitmap?>(null)

    /** How far the dissolve has gone: the new record's opacity, or the old one's absence when there is none. */
    internal val fade = Animatable(1f)

    /** The drift: a breath in scale, a turn in degrees, and travel across the page from -1 to 1. */
    internal val scale = Animatable(1f)
    internal val turn = Animatable(0f)
    internal val travel = Animatable(0f)

    /**
     * Swap in a new record, dissolving to it over long enough that you notice
     * the room has changed colour without catching it changing. A record with
     * nothing to show fades the old one out, leaving the bare ground.
     */
    internal suspend fun install(baked: ImageBitmap?) {
        if (baked == current && previous == null) return
        previous = current ?: previous
        current = baked
        fade.snapTo(0f)
        fade.animateTo(1f, tween(DISSOLVE_MS, easing = FastOutSlowInEasing))
        // At rest only one texture is drawn.
        previous = null
    }

    /**
     * Breathing: three incommensurate periods, so the drift never arrives back
     * where it started and never reads as a loop. From wherever it stands.
     */
    internal suspend fun breathe() = coroutineScope {
        launch { swing(scale, 1f, FAR / NEAR, SCALE_MS) }
        launch { swing(turn, -TURN, TURN, TURN_MS) }
        launch { swing(travel, -1f, 1f, TRAVEL_MS) }
    }

    /** Coming to rest from wherever the drift had reached, rather than stopping dead. */
    internal suspend fun settle() = coroutineScope {
        for ((value, rest) in listOf(scale to 1f, turn to 0f, travel to 0f)) {
            launch { value.animateTo(rest, tween(DISSOLVE_MS, easing = FastOutSlowInEasing)) }
        }
    }

    private suspend fun swing(value: Animatable<Float, *>, low: Float, high: Float, period: Int) {
        while (true) {
            value.animateTo(high, tween(period, easing = FastOutSlowInEasing))
            value.animateTo(low, tween(period, easing = FastOutSlowInEasing))
        }
    }

    internal companion object {
        /**
         * The overscan the texture is drawn at, and how far the drift travels in
         * it. What decides whether motion is seen is how far it goes against how
         * soft the thing moving is; these reach about four of the wash's softest
         * features, and `NEAR` keeps the texture's edge out of frame throughout.
         */
        const val NEAR = 1.38f
        const val FAR = 1.58f
        const val REACH = 0.12f
        const val RISE = 0.10f
        const val TURN = 3f
        const val SCALE_MS = 13_000
        const val TURN_MS = 19_000
        const val TRAVEL_MS = 23_000
        const val DISSOLVE_MS = 2_000

        /** The wash's strength over the ground: the theme's, or the platform look's lighter one. */
        fun strength(koan: Boolean) = if (koan) 0.6f else 0.5f
    }
}

internal val LocalRecordWash = staticCompositionLocalOf<RecordWash?> { null }

/**
 * The wash for everything inside, from the playing record's decoded sleeve.
 * `pending` is a sleeve still being fetched, which is not a record without
 * one: the room keeps what it is wearing until it is answered. Colours from
 * the record off, there is none.
 */
@Composable
fun ProvideRecordWash(
    sleeve: Bitmap?,
    pending: Boolean,
    playing: Boolean,
    recordColours: Boolean,
    graphics: Graphics = Graphics.Full,
    tint: Int = 100,
    content: @Composable () -> Unit,
) {
    val wash = remember { RecordWash() }
    val look = KoanTheme.look
    val tone = if (look.dark) WashTone.Dark else WashTone.Light
    val ground = (if (look.isOn) look.colors.bg else MaterialTheme.colorScheme.background).toArgb()
    val strength = RecordWash.strength(look.isOn)
    // The theme tint is the kōan theme's; the platform's look holds the wash fully.
    val hold = if (look.isOn) tint / 100.0 else 1.0
    val shown = WashRules.shown(recordColours, graphics)
    LaunchedEffect(sleeve, pending, tone, ground, shown, hold) {
        if (pending && shown) return@LaunchedEffect
        val baked = sleeve?.takeIf { shown }?.let {
            withContext(Dispatchers.Default) { bake(it, tone, ground, strength, hold) }
        }
        wash.install(baked)
    }

    val context = LocalContext.current
    val lifecycle by LocalLifecycleOwner.current.lifecycle.currentStateAsState()
    val visible = lifecycle.isAtLeast(Lifecycle.State.STARTED)
    // Read again each time the app comes back into view: both are system
    // settings changed outside it.
    val animations = remember(visible) { ValueAnimator.areAnimatorsEnabled() }
    val powerSaving = remember(visible) { context.getSystemService(PowerManager::class.java)?.isPowerSaveMode == true }
    val drifts = shown && WashRules.drifts(playing, visible, animations, powerSaving, graphics)
    LaunchedEffect(drifts) { if (drifts) wash.breathe() else wash.settle() }

    CompositionLocalProvider(LocalRecordWash provides wash, content = content)
}

private fun bake(sleeve: Bitmap, tone: WashTone, ground: Int, strength: Float, hold: Double): ImageBitmap {
    val side = WashBake.SIDE
    val small = sleeve.scale(side, side)
    val pixels = IntArray(side * side)
    small.getPixels(pixels, 0, side, 0, 0, side, side)
    if (small !== sleeve) small.recycle()
    val baked = WashBake.bake(pixels, side, tone, ground, strength, hold)
    return Bitmap.createBitmap(baked, side, side, Bitmap.Config.ARGB_8888).asImageBitmap()
}

/**
 * The page's ground with the wash on it: the theme's `bg`, or the platform's
 * background, and the playing record over it where there is one. Draws in a
 * layer of its own below the content's, so the drift redraws the wash and
 * never the page.
 */
@Composable
fun Modifier.recordWash(): Modifier {
    val wash = LocalRecordWash.current
    val ground = if (KoanTheme.isOn) KoanTheme.colors.bg else MaterialTheme.colorScheme.background
    return clipToBounds()
        .drawBehind {
            drawRect(ground)
            if (wash != null) drawWash(wash)
        }
        .graphicsLayer()
}

private fun DrawScope.drawWash(wash: RecordWash) {
    val current = wash.current
    val previous = wash.previous
    if (current == null && previous == null) return
    val fade = wash.fade.value
    val travel = wash.travel.value
    withTransform({
        translate(size.width * RecordWash.REACH * travel, size.height * RecordWash.RISE * travel)
        rotate(wash.turn.value, center)
        scale(wash.scale.value, wash.scale.value, center)
    }) {
        previous?.let { texture(it, if (current != null) 1f else 1f - fade) }
        current?.let { texture(it, fade) }
    }
}

/** A square texture filling the page with room to move, centred. */
private fun DrawScope.texture(image: ImageBitmap, alpha: Float) {
    if (alpha <= 0f) return
    val side = (max(size.width, size.height) * RecordWash.NEAR).roundToInt()
    image.let {
        drawImage(
            it,
            dstOffset = IntOffset(((size.width - side) / 2).roundToInt(), ((size.height - side) / 2).roundToInt()),
            dstSize = IntSize(side, side),
            alpha = alpha,
            filterQuality = FilterQuality.Low,
        )
    }
}
