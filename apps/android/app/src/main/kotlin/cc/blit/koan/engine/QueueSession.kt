package cc.blit.koan.engine

import cc.blit.koan.state.LyricsModel
import cc.blit.koan.state.PlayingLevels
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.SupervisorJob
import java.util.WeakHashMap

/**
 * The queue page, mini player and now-playing sheet's own long-lived objects,
 * one set per [KoanRuntime]: the levels subscription every playing indicator
 * shares, and the lyrics model.
 *
 * They need the engine, which a composable never touches, so they are built
 * here, once, off the composition and handed to it already made. Held weakly
 * by runtime: a new runtime gets its own, and none outlives it.
 */
object QueueSession {
    private class Held(val levels: PlayingLevels, val lyrics: LyricsModel)

    private val held = WeakHashMap<KoanRuntime, Held>()

    private fun of(runtime: KoanRuntime): Held = synchronized(held) {
        held.getOrPut(runtime) {
            val scope = CoroutineScope(SupervisorJob() + EngineDispatcher.dispatcher)
            val engine = runtime.engine
            Held(
                PlayingLevels(scope, open = { engine.vizStream() }, setFps = { engine.setVizFps(it.toUByte()) }),
                LyricsModel(engine, runtime.mirror, EngineDispatcher.dispatcher),
            )
        }
    }

    fun levels(runtime: KoanRuntime): PlayingLevels = of(runtime).levels

    fun lyrics(runtime: KoanRuntime): LyricsModel = of(runtime).lyrics
}
