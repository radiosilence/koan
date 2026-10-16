package cc.blit.koan.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.unit.dp
import cc.blit.koan.engine.Tasks
import cc.blit.koan.ffi.SyncPhase
import cc.blit.koan.ffi.SyncProgress
import cc.blit.koan.ui.theme.KoanEdge
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import cc.blit.koan.ui.theme.koanRule
import cc.blit.koan.ui.theme.koanSurface
import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update

/** Something slow happening now, as the activity card shows it. */
@Immutable
data class ActivityTask(
    val id: Long,
    /** "Signing in to music.example.com", "Syncing with server". */
    val label: String,
    /** A remote sync, whose counts come from the engine's sync progress. */
    val followsSync: Boolean = false,
    /** Started by the engine on its own, and ended when it says so. */
    val mirrored: Boolean = false,
    /** What `done` and `total` count, where saying so reads better: "12,400 of 49,700 tracks". */
    val unit: String? = null,
    val done: Long? = null,
    val total: Long? = null,
) {
    /** 0…1, or `null` with nothing to divide by. */
    val progress get() = total?.takeIf { it > 0 }?.let { t -> done?.let { (it.toFloat() / t).coerceIn(0f, 1f) } }

    /** "12,400 of 49,700 tracks" where the task says what it counts, "12,345 / 48,087" where it does not. */
    val counts: String?
        get() {
            val d = done ?: return null
            val unit = unit?.let { " $it" } ?: ""
            val t = total?.takeIf { it > 0 } ?: return "%,d".format(d) + unit
            return "%,d".format(d) + (if (this.unit == null) " / " else " of ") + "%,d".format(t) + unit
        }
}

/**
 * Everything slow that is happening now: the counterpart of the Apple apps'
 * `ActivityModel`. A model wraps its slow work in `run`; what the engine
 * starts by itself (an automatic sync, a scan) is followed from its own
 * state, never polled. One for the app, so the card shows it on any page.
 */
class ActivityModel {
    private val _tasks = MutableStateFlow<List<ActivityTask>>(emptyList())
    val tasks: StateFlow<List<ActivityTask>> = _tasks.asStateFlow()
    private val ids = AtomicLong()

    /** Run `work` with a task shown for as long as it takes, however it ends. */
    suspend fun <T> run(label: String, followsSync: Boolean = false, work: suspend () -> T): T {
        val id = ids.incrementAndGet()
        _tasks.update { it + ActivityTask(id, label, followsSync) }
        try {
            return work()
        } finally {
            _tasks.update { tasks -> tasks.filterNot { it.id == id } }
        }
    }

    /**
     * What the engine is doing on its own: a sync, a scan. Shown while it
     * says so, and not twice: a sync this app asked for already has a row,
     * which takes the engine's counts.
     */
    fun follow(tasks: Tasks, sync: SyncProgress?) {
        _tasks.update { current ->
            val asked = current.filterNot { it.mirrored }
            fun kept(label: String, followsSync: Boolean = false) =
                current.firstOrNull { it.mirrored && it.label == label }
                    ?: ActivityTask(ids.incrementAndGet(), label, followsSync, mirrored = true)
            val mirrored = listOfNotNull(
                kept(SYNCING, followsSync = true).takeIf { tasks.syncing && asked.none { it.followsSync } },
                kept(SCANNING).takeIf { tasks.scanning },
            )
            (asked + mirrored).map { if (it.followsSync) withSync(it, sync) else it }
        }
    }

    private fun withSync(task: ActivityTask, sync: SyncProgress?): ActivityTask {
        if (sync == null) return task.copy(unit = null, done = null, total = null)
        val unit = when (sync.phase) {
            SyncPhase.ALBUMS -> "records"
            SyncPhase.TRACKS -> "tracks"
            SyncPhase.ARTISTS -> "artists"
            SyncPhase.FINISHING -> null
        }
        return task.copy(unit = unit, done = sync.done.toLong(), total = sync.total?.toLong())
    }

    companion object {
        const val SYNCING = "Syncing with server"
        const val SCANNING = "Scanning library"
    }
}

/**
 * What the app is busy with, floating over every page just above the
 * transport, as iOS's card does. Not there when idle, so it is not furniture.
 */
@Composable
fun ActivityCard(tasks: List<ActivityTask>, modifier: Modifier = Modifier) {
    if (tasks.isEmpty()) return
    Column(
        modifier
            .padding(horizontal = KoanSpace.m, vertical = KoanSpace.s)
            .fillMaxWidth()
            .koanSurface()
            .koanRule(KoanEdge.Top)
            .padding(KoanSpace.m),
        verticalArrangement = Arrangement.spacedBy(KoanSpace.s),
    ) {
        tasks.forEach { task ->
            Column(verticalArrangement = Arrangement.spacedBy(KoanSpace.xs)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    KoanText(task.label, Modifier.weight(1f), role = KoanType.Fine, tone = KoanTone.Muted, maxLines = 1)
                    task.counts?.let {
                        Spacer(Modifier.width(KoanSpace.xs))
                        KoanText(it, role = KoanType.Fine, tone = KoanTone.Muted, maxLines = 1)
                    }
                }
                // Determinate where the engine counts, moving where it cannot:
                // the same height, so the row does not jump when a total arrives.
                val colour = KoanTheme.color(KoanTone.Accent)
                val track = KoanTheme.color(KoanTone.Rule)
                val bar = Modifier.fillMaxWidth().height(2.dp)
                val progress = task.progress
                if (progress != null) {
                    LinearProgressIndicator(
                        progress = { progress }, modifier = bar, color = colour, trackColor = track,
                        strokeCap = StrokeCap.Butt, gapSize = 0.dp, drawStopIndicator = {},
                    )
                } else {
                    LinearProgressIndicator(bar, color = colour, trackColor = track, strokeCap = StrokeCap.Butt, gapSize = 0.dp)
                }
            }
        }
    }
}
