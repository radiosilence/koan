package cc.blit.koan.state

import cc.blit.koan.record
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.koan_ffi.SyncPhase
import uniffi.koan_ffi.SyncProgress

@OptIn(ExperimentalCoroutinesApi::class)
class ActivityModelTest {
    @Test
    fun `only what would touch the same is in the way`() {
        val activity = ActivityModel()
        val sync = activity.begin("Syncing with server", setOf(Resource.RemoteTracks))
        assertTrue(activity.conflicts(setOf(Resource.RemoteTracks)))
        assertFalse("a scan writes your files' rows, not the server's", activity.conflicts(Resource.LocalLibrary))
        assertTrue(activity.conflicts(Resource.WholeLibrary))
        activity.end(sync)
        assertFalse(activity.conflicts(Resource.WholeLibrary))
    }

    @Test
    fun `busy changes when a task starts or ends and not on a progress tick`() = runTest {
        val activity = ActivityModel()
        val seen = activity.busy.record(backgroundScope)
        val id = activity.begin("Scanning library", Resource.LocalLibrary, cancellable = true)
        val other = activity.begin("Queue add") // holds nothing
        assertEquals(2, seen.size)
        activity.setCounts(id, 1, 10)
        activity.setDetail(id, "a.flac")
        assertEquals(2, seen.size)
        activity.end(other)
        assertEquals(2, seen.size)
        activity.end(id)
        assertEquals(setOf<Resource>(), seen.last())
        assertEquals(3, seen.size)
    }

    @Test
    fun `a task's fraction is what its counts say, and nothing when there is nothing to divide by`() {
        val activity = ActivityModel()
        val id = activity.begin("Scanning library")
        assertNull(activity.tasks.value.single().progress)
        activity.setCounts(id, 5, null)
        assertNull(activity.tasks.value.single().progress)
        activity.setCounts(id, 5, 20)
        assertEquals(0.25, activity.tasks.value.single().progress!!, 1e-9)
        activity.setCounts(id, 30, 20)
        assertEquals(1.0, activity.tasks.value.single().progress!!, 0.0)
        activity.setCounts(id, 0, 0)
        assertNull(activity.tasks.value.single().progress)
    }

    @Test
    fun `a task ends however its work does`() = runTest {
        val activity = ActivityModel()
        val failed = activity.run("Sync", Resource.LocalLibrary) { error("boom") }
        assertTrue(failed.isFailure)
        assertEquals(emptyList<ActivityTask>(), activity.tasks.value)
        assertEquals(emptySet<Resource>(), activity.busy.value)

        val fine = activity.run("Sync") { activity.tasks.value.single().label }
        assertEquals("Sync", fine.getOrThrow())
        assertEquals(emptyList<ActivityTask>(), activity.tasks.value)
    }

    @Test
    fun `the engine's reporter puts counts and detail on the task, and no total is none`() = runTest {
        val activity = ActivityModel()
        activity.runReporting("Scanning library", Resource.LocalLibrary) { reporter ->
            reporter.started(0uL)
            reporter.advanced(3uL, "a.flac")
            val unknown = activity.tasks.value.single()
            assertEquals(3L, unknown.done)
            assertNull("zero is unknowable", unknown.progress)
            assertEquals("a.flac", unknown.detail)
            reporter.started(100uL)
            reporter.advanced(40uL, "b.flac")
            assertEquals(0.4, activity.tasks.value.single().progress!!, 1e-9)
        }
    }

    @Test
    fun `cancel stops the scan only from a row that is a scan`() {
        val activity = ActivityModel()
        var cancelled = 0
        activity.cancelLibraryTask = { cancelled++ }
        val sync = activity.begin("Syncing with server", setOf(Resource.RemoteTracks))
        val scan = activity.begin("Scanning library", Resource.LocalLibrary, cancellable = true)
        activity.cancel(sync)
        assertEquals(0, cancelled)
        activity.cancel(scan)
        assertEquals(1, cancelled)
    }

    @Test
    fun `an engine job shows a row for as long as it runs, once`() {
        val activity = ActivityModel()
        activity.mirror("Scanning library", running = true, uses = Resource.LocalLibrary)
        activity.mirror("Scanning library", running = true, uses = Resource.LocalLibrary)
        assertEquals(1, activity.tasks.value.size)
        activity.mirror("Scanning library", running = false)
        activity.mirror("Scanning library", running = false)
        assertEquals(0, activity.tasks.value.size)
    }

    @Test
    fun `sync progress lands on every sync row and on no other`() {
        val activity = ActivityModel()
        activity.begin("Scanning library", Resource.LocalLibrary)
        activity.begin("Syncing with server", setOf(Resource.RemoteTracks), followsSync = true)
        activity.showSync(SyncProgress(SyncPhase.ALBUMS, 12uL, null))
        var sync = activity.tasks.value.single { it.followsSync }
        assertEquals("albums", sync.unit)
        assertEquals("Listing albums", sync.detail)
        activity.showSync(SyncProgress(SyncPhase.TRACKS, 500uL, 2000uL))
        sync = activity.tasks.value.single { it.followsSync }
        assertEquals(0.25, sync.progress!!, 1e-9)
        assertNull(sync.detail)
        activity.showSync(SyncProgress(SyncPhase.FINISHING, 0uL, null))
        sync = activity.tasks.value.single { it.followsSync }
        assertEquals("Finishing", sync.detail)
        assertNull(sync.done)
        activity.showSync(null)
        assertEquals("Finishing", activity.tasks.value.single { it.followsSync }.detail)
        assertNull(activity.tasks.value.single { !it.followsSync }.unit)
    }
}
