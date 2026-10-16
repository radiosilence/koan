package cc.blit.koan.engine

import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.asCoroutineDispatcher
import java.util.concurrent.Executors

/**
 * Where every call into the engine runs. Never `Dispatchers.Main`: a call
 * crosses JNA, and a UI thread that waits on the engine drops frames.
 *
 * One thread, deliberately. The calls are `suspend` and do not hold it while
 * the engine works, so one thread is no bottleneck; and it makes the order in
 * which commands are *issued* the order they were asked for, which the
 * engine's own ordered lane (`offload.rs`) then preserves. A pool could issue
 * "pause" and "resume" the wrong way round.
 */
object EngineDispatcher {
    val dispatcher: CoroutineDispatcher =
        Executors.newSingleThreadExecutor { runnable ->
            Thread(runnable, "koan-engine-calls").apply { isDaemon = true }
        }.asCoroutineDispatcher()
}
