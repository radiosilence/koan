package cc.blit.koan.ui.library

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import cc.blit.koan.engine.EngineMirror
import cc.blit.koan.ffi.VizLevels
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.theme.KoanTheme
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.stateIn

/**
 * The music's level in three bands, for every playing indicator on screen.
 *
 * One analyser subscription, held only while a bar is drawn: the analyser
 * publishes nothing while paused, and parks with no one reading.
 */
private object Levels {
    private var held: StateFlow<VizLevels?>? = null

    fun of(mirror: EngineMirror): StateFlow<VizLevels?> = synchronized(this) {
        held ?: flow {
            mirror.engine().vizStream().use { stream ->
                while (true) emit(stream.next() ?: break)
            }
        }
            .stateIn(CoroutineScope(SupervisorJob() + Dispatchers.Default), SharingStarted.WhileSubscribed(), null)
            .also { held = it }
    }
}

/**
 * Three bars where a playing track's number would be, moving with the music
 * while it plays and resting low while it is paused. The levels are read in
 * the draw phase, so a frame redraws the bars and recomposes nothing.
 */
@Composable
fun PlayingBars(playing: Boolean, modifier: Modifier = Modifier, size: Dp = 14.dp, color: Color = KoanTheme.look.accent) {
    val levels = if (playing) Levels.of(koanApp().mirror).collectAsState() else null
    Canvas(modifier.size(size)) {
        val l = levels?.value
        val heights = if (l == null) listOf(0.3f, 0.5f, 0.4f) else listOf(l.low, l.mid, l.high).map { it.coerceIn(0.12f, 1f) }
        val gap = this.size.width / 7
        val bar = (this.size.width - gap * 2) / 3
        heights.forEachIndexed { i, h ->
            val height = this.size.height * h
            drawRect(
                color,
                topLeft = Offset(i * (bar + gap), this.size.height - height),
                size = Size(bar, height),
            )
        }
    }
}
