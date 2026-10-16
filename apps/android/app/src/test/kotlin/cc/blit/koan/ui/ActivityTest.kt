package cc.blit.koan.ui

import cc.blit.koan.engine.Tasks
import cc.blit.koan.ffi.SyncPhase
import cc.blit.koan.ffi.SyncProgress
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** What the activity card lists: work the app asked for, and what the engine started itself. */
class ActivityTest {
    private val idle = Tasks(scanning = false, syncing = false)

    @Test fun aTaskIsShownForAsLongAsItRuns() = runTest {
        val model = ActivityModel()
        val gate = CompletableDeferred<Unit>()
        val job = launch { model.run("Clearing downloads") { gate.await() } }
        runCurrent()
        assertEquals(listOf("Clearing downloads"), model.tasks.value.map { it.label })
        gate.complete(Unit)
        job.join()
        assertTrue(model.tasks.value.isEmpty())
    }

    @Test fun aFailedTaskIsTakenOffToo() = runTest {
        val model = ActivityModel()
        runCatching { model.run("Signing in") { error("refused") } }
        assertTrue(model.tasks.value.isEmpty())
    }

    @Test fun anEngineSyncIsMirroredWithItsCounts() {
        val model = ActivityModel()
        model.follow(Tasks(scanning = false, syncing = true), SyncProgress(SyncPhase.TRACKS, 12_400uL, 49_700uL))
        val task = model.tasks.value.single()
        assertEquals(ActivityModel.SYNCING, task.label)
        assertEquals("12,400 of 49,700 tracks", task.counts)
        assertEquals(12_400f / 49_700f, task.progress!!, 1e-6f)
    }

    @Test fun aMirroredRowKeepsItsIdAcrossProgress() {
        val model = ActivityModel()
        model.follow(Tasks(scanning = true, syncing = true), SyncProgress(SyncPhase.ALBUMS, 10uL, null))
        val ids = model.tasks.value.map { it.id }
        model.follow(Tasks(scanning = true, syncing = true), SyncProgress(SyncPhase.TRACKS, 100uL, 200uL))
        assertEquals(ids, model.tasks.value.map { it.id })
        model.follow(idle, null)
        assertTrue(model.tasks.value.isEmpty())
    }

    @Test fun aSyncAskedForIsNotShownTwice() = runTest {
        val model = ActivityModel()
        val gate = CompletableDeferred<Unit>()
        val job = launch { model.run("Loading the library", followsSync = true) { gate.await() } }
        runCurrent()
        model.follow(Tasks(scanning = false, syncing = true), SyncProgress(SyncPhase.ALBUMS, 300uL, null))
        val task = model.tasks.value.single()
        assertEquals("Loading the library", task.label)
        assertEquals("300 records", task.counts)
        assertNull(task.progress)
        gate.complete(Unit)
        job.join()
    }

    @Test fun countsWithoutAUnitAreAFraction() {
        assertEquals("12,345 / 48,087", ActivityTask(1, "x", done = 12_345, total = 48_087).counts)
        assertNull(ActivityTask(1, "x").counts)
    }
}
