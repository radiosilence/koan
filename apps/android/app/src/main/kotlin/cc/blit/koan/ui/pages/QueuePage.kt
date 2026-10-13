package cc.blit.koan.ui.pages

import androidx.compose.runtime.Composable
import cc.blit.koan.engine.KoanRuntime

/** The queue (the Queue tab's root). */
@Composable
fun QueuePage(runtime: KoanRuntime) {
    PlaceholderPage(
        "queue",
        "runtime.mirror.queue and queueRows (LazyColumn keyed by queueItemId), playback, playhead; runtime.player for every command.",
    )
}
