package cc.blit.koan.state

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import uniffi.koan_ffi.ProgressReporter
import uniffi.koan_ffi.SyncPhase
import uniffi.koan_ffi.SyncProgress
import java.util.UUID
import java.util.concurrent.atomic.AtomicLong

/**
 * What a task has hold of, so the next one can be told apart from the ones it
 * would collide with.
 *
 * Every library task queues behind SQLite's one writer, but most of them never
 * meet (a sync writes the server's rows while a scan writes your files') and
 * sitting out a twenty-minute sync before you can rescan a folder buys
 * nothing. So a task says what it touches and only what would touch the same
 * is disabled.
 */
enum class Resource {
    /** The files on disk: read for their tags, or moved. */
    LocalFiles,

    /** Library rows for files on this machine. */
    LocalTracks,

    /** Library rows mirrored from the server. */
    RemoteTracks,

    /** The downloaded copies of remote tracks. */
    Downloads,
    ;

    companion object {
        /** Reading files and writing their rows: a scan, a drop, a move. */
        val LocalLibrary: Set<Resource> = setOf(LocalFiles, LocalTracks)

        /** For the tasks that empty the library or rewrite all of it. */
        val WholeLibrary: Set<Resource> = setOf(LocalFiles, LocalTracks, RemoteTracks, Downloads)
    }
}

/** One thing koan is busy with. */
data class ActivityTask(
    val id: String,
    /** Shown to the user: "Scanning library", "Syncing with server". */
    val label: String,
    /**
     * What starting another task would have to wait for. Empty for work small
     * enough that nothing need wait on it: a queue edit, a sign-in.
     */
    val uses: Set<Resource> = emptySet(),
    /** Whether asking it to stop does anything. */
    val cancellable: Boolean = false,
    /** A remote sync, whose counts come from the engine's sync progress rather than a reporter of its own. */
    val followsSync: Boolean = false,
    /** What [done] and [total] count, when saying so reads better than a bare pair of numbers. */
    val unit: String? = null,
    val done: Long? = null,
    val total: Long? = null,
    /** What it is working on right now, if it says. */
    val detail: String? = null,
) {
    /** 0 to 1, or null when there is nothing to divide by. */
    val progress: Double?
        get() {
            val d = done ?: return null
            val t = total ?: return null
            return if (t > 0) minOf(1.0, d.toDouble() / t) else null
        }
}

/**
 * Everything slow that is happening right now. The port of
 * `ActivityModel.swift`, whose doc comments are the specification.
 *
 * A scan, a remote sync, a large queue add and a library rebuild all take
 * anywhere from a second to a minute, and without a sign of them pressing the
 * button looks like nothing happened. One registry, one place to show it, and
 * a new kind of task calls [run] rather than inventing another boolean.
 *
 * Safe from any thread: every change is an atomic update of a flow, which is
 * what the engine's progress callbacks (from its worker threads) need.
 */
class ActivityModel {
    private val _tasks = MutableStateFlow<List<ActivityTask>>(emptyList())
    private val _busy = MutableStateFlow<Set<Resource>>(emptySet())
    private val mirrored = HashMap<String, String>()

    val tasks: StateFlow<List<ActivityTask>> = _tasks.asStateFlow()

    /**
     * Everything the running tasks between them have hold of. Its own flow, not
     * derived by a reader of [tasks]: a menu that gates on it should not be
     * woken by every progress tick, only when a task starts or ends.
     */
    val busy: StateFlow<Set<Resource>> = _busy.asStateFlow()

    /** Called to stop the running scan. Set by whoever owns the engine. */
    @Volatile
    var cancelLibraryTask: (() -> Unit)? = null

    /**
     * Whether starting something that needs [wanted] would collide with what is
     * already running. What every menu item and button gates on.
     */
    fun conflicts(wanted: Set<Resource>): Boolean = _busy.value.any { it in wanted }

    /**
     * The scanner is the only thing that reads the engine's cancel flag, so
     * only a scan is ever marked cancellable: a cancel button on any other row
     * would stop a scan running beside it instead of the task it sits on.
     */
    fun cancel(id: String) {
        if (_tasks.value.firstOrNull { it.id == id }?.cancellable == true) cancelLibraryTask?.invoke()
    }

    /**
     * Run [work] with a task registered for its duration, removed however it
     * ends, including by throwing, so a failure cannot leave a spinner running
     * forever. Cancellation is rethrown, not captured.
     */
    suspend fun <T> run(
        label: String,
        uses: Set<Resource> = emptySet(),
        followsSync: Boolean = false,
        work: suspend () -> T,
    ): Result<T> {
        val id = begin(label, uses, followsSync = followsSync)
        return try {
            Result.success(work())
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            Result.failure(e)
        } finally {
            end(id)
        }
    }

    /** Run [work] with a reporter the engine can call back into, so the task shows a fraction rather than a spinner. */
    suspend fun <T> runReporting(
        label: String,
        uses: Set<Resource> = emptySet(),
        cancellable: Boolean = true,
        work: suspend (ProgressReporter) -> T,
    ): Result<T> {
        val id = begin(label, uses, cancellable = cancellable)
        return try {
            Result.success(work(reporter(id)))
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            Result.failure(e)
        } finally {
            end(id)
        }
    }

    /**
     * For work that does not fit the closure shape: a long-lived poller, or
     * something whose lifetime is owned elsewhere. Pair every [begin] with an [end].
     */
    fun begin(
        label: String,
        uses: Set<Resource> = emptySet(),
        cancellable: Boolean = false,
        followsSync: Boolean = false,
    ): String {
        val task = ActivityTask(UUID.randomUUID().toString(), label, uses, cancellable, followsSync)
        _tasks.update { it + task }
        refreshBusy()
        return task.id
    }

    fun end(id: String) {
        _tasks.update { list -> list.filterNot { it.id == id } }
        refreshBusy()
    }

    private fun refreshBusy() {
        // Equal sets are not a change, so nothing waiting on [busy] wakes.
        _busy.value = _tasks.value.flatMapTo(HashSet()) { it.uses }
    }

    fun setCounts(id: String, done: Long?, total: Long?) = edit(id) {
        it.copy(done = done ?: it.done, total = total ?: it.total)
    }

    fun setDetail(id: String, detail: String?) = edit(id) { it.copy(detail = detail) }

    private fun edit(id: String, change: (ActivityTask) -> ActivityTask) {
        _tasks.update { list -> list.map { if (it.id == id) change(it) else it } }
    }

    /**
     * Mirror a background job the engine runs on its own (the startup scan,
     * the folder watcher, the automatic sync). They are not started by the UI,
     * so nothing here can wrap them: the engine says whether each is running
     * and this shows a row for as long as it is. Called with each answer as it
     * arrives rather than asking for it.
     */
    fun mirror(
        label: String,
        running: Boolean,
        uses: Set<Resource> = emptySet(),
        cancellable: Boolean = false,
        followsSync: Boolean = false,
    ) {
        synchronized(mirrored) {
            val existing = mirrored[label]
            when {
                running && existing == null -> mirrored[label] = begin(label, uses, cancellable, followsSync)
                !running && existing != null -> {
                    end(existing)
                    mirrored.remove(label)
                }
            }
        }
    }

    /**
     * Put the engine's sync progress on every sync row. There is one sync
     * progress however the sync was started, so the row for a sync asked for
     * and the row for the automatic one read the same thing. Null between
     * syncs leaves the rows as they are; they end on their own.
     */
    fun showSync(progress: SyncProgress?) {
        if (progress == null) return
        _tasks.update { list ->
            list.map { task ->
                if (!task.followsSync) {
                    task
                } else {
                    when (progress.phase) {
                        SyncPhase.ALBUMS -> task.copy(
                            done = progress.done.toLong(), total = null, unit = "albums", detail = "Listing albums",
                        )
                        SyncPhase.TRACKS -> task.copy(
                            done = progress.done.toLong(), total = progress.total?.toLong(), unit = "tracks", detail = null,
                        )
                        SyncPhase.ARTISTS -> task.copy(done = null, total = null, unit = null, detail = "Recording artists")
                        SyncPhase.FINISHING -> task.copy(done = null, total = null, unit = null, detail = "Finishing")
                    }
                }
            }
        }
    }

    /**
     * A reporter the engine can call from its worker threads. koan already
     * throttles the calls, roughly one per sixty-odd files, so this does not
     * throttle again.
     */
    fun reporter(id: String): ProgressReporter = EngineProgress(this, id)
}

/** Bridges the engine's progress callbacks onto the model. */
internal class EngineProgress(private val activity: ActivityModel, private val task: String) : ProgressReporter {
    private val total = AtomicLong(0)

    override fun started(total: ULong) {
        this.total.set(total.toLong())
        activity.setCounts(task, done = 0, total = total.toLong())
    }

    override fun advanced(done: ULong, detail: String) {
        val total = total.get()
        activity.setCounts(task, done = done.toLong(), total = if (total > 0) total else null)
        activity.setDetail(task, detail)
    }
}
