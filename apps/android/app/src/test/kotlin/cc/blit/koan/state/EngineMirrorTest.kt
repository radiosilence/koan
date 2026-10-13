package cc.blit.koan.state

import cc.blit.koan.FakeStream
import cc.blit.koan.item
import cc.blit.koan.nowPlaying
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.koan_ffi.EntryStatus
import uniffi.koan_ffi.PlayState
import uniffi.koan_ffi.StateSlice

@OptIn(ExperimentalCoroutinesApi::class)
class EngineMirrorTest {
    private var now = 1_000_000_000L
    private val mirror = EngineMirror(nanoClock = { now })

    /** How many distinct values `flow` emits while `body` runs: what a collector would be woken for. */
    private fun <T> kotlinx.coroutines.test.TestScope.emissions(flow: StateFlow<T>, body: () -> Unit): Int {
        val seen = mutableListOf<T>()
        val job = launch(UnconfinedTestDispatcher(testScheduler)) { flow.toList(seen) }
        body()
        job.cancel()
        return seen.size - 1 // the initial value is not a change
    }

    @Test
    fun `a slice equal to the one held does not wake a collector`() = runTest {
        val playing = nowPlaying(PlayState.PLAYING, item("a", 1))
        mirror.apply(StateSlice.Playback(playing))
        val woken = emissions(mirror.playback) {
            mirror.apply(StateSlice.Playback(playing.copy())) // equal, new instance
            mirror.apply(StateSlice.Playback(playing.copy(state = PlayState.PAUSED)))
        }
        assertEquals(1, woken)
        assertEquals(PlayState.PAUSED, mirror.playback.value.state)
    }

    @Test
    fun `tasks and cache and library emit only when they moved`() = runTest {
        mirror.apply(StateSlice.Tasks(scanning = true, syncing = false))
        mirror.apply(StateSlice.Cache(10uL))
        mirror.apply(StateSlice.Library(3uL))
        assertEquals(0, emissions(mirror.tasks) { mirror.apply(StateSlice.Tasks(true, false)) })
        assertEquals(0, emissions(mirror.cacheBytes) { mirror.apply(StateSlice.Cache(10uL)) })
        assertEquals(0, emissions(mirror.libraryVersion) { mirror.apply(StateSlice.Library(3uL)) })
        assertEquals(1, emissions(mirror.libraryVersion) { mirror.apply(StateSlice.Library(4uL)) })
        assertEquals(EngineTasks(true, false), mirror.tasks.value)
    }

    @Test
    fun `a playhead is an anchor the position runs on from`() {
        mirror.apply(StateSlice.Playhead(positionMs = 5_000uL, seekableMs = 180_000uL, playing = true))
        val head = mirror.playhead.value
        assertEquals(5_000L, head.at(now))
        assertEquals(7_500L, head.at(now + 2_500_000_000L))
        // Not past the end of the track, while the engine is yet to say the next has begun.
        assertEquals(10_000L, head.at(now + 60_000_000_000L, withinMs = 10_000))
        assertEquals(180_000L, mirror.seekableMs.value)

        mirror.apply(StateSlice.Playhead(5_000uL, 180_000uL, playing = false))
        assertEquals(5_000L, mirror.playhead.value.at(now + 9_000_000_000L))
    }

    @Test
    fun `the seekable extent is its own flow and moves only when it does`() = runTest {
        mirror.apply(StateSlice.Playhead(0uL, 1_000uL, true))
        val woken = emissions(mirror.seekableMs) {
            now += 1
            mirror.apply(StateSlice.Playhead(0uL, 1_000uL, true)) // re-anchored, extent unchanged
        }
        assertEquals(0, woken)
    }

    @Test
    fun `a queue sent whole seeds rows, the queue and both indexes`() {
        val rows = listOf(item("a", 1, entry = 10), item("b", 2, entry = 11), item("c", 1, entry = 12))
        mirror.apply(StateSlice.Queue(rows, 7uL))
        assertEquals(7L, mirror.queueRows.value.version)
        assertEquals(rows, mirror.queue.value.items)
        assertEquals("b", mirror.queue.value.item("b")?.queueItemId)
        assertNull(mirror.queue.value.item("zzz"))
        assertEquals(2, mirror.queue.value.indexOf("c"))
        // One track queued twice is one entry in the by-track index; by playlist row it is both.
        assertEquals(setOf(1L, 2L), mirror.queueIndex.value.byTrack.keys)
        assertEquals(setOf(10L, 11L, 12L), mirror.queueIndex.value.byPlaylistEntry.keys)
    }

    @Test
    fun `a track queued twice prefers the entry doing something`() {
        val rows = listOf(item("a", 1), item("b", 1))
        mirror.apply(StateSlice.Queue(rows, 1uL))
        assertEquals("a", mirror.queueIndex.value.byTrack[1]?.queueItemId)
        mirror.apply(StateSlice.QueuePatch(1uL, listOf(item("b", 1, status = EntryStatus.PLAYING))))
        assertEquals("b", mirror.queueIndex.value.byTrack[1]?.queueItemId)
    }

    @Test
    fun `a patch changes rows but not the queue rows, and is relative to the base`() = runTest {
        val rows = listOf(item("a", 1), item("b", 2), item("c", 3))
        mirror.apply(StateSlice.Queue(rows, 1uL))
        val baseRows = mirror.queueRows.value

        val woken = emissions(mirror.queueRows) {
            mirror.apply(StateSlice.QueuePatch(1uL, listOf(item("a", 1, status = EntryStatus.PLAYING))))
        }
        assertEquals(0, woken)
        assertSame(baseRows, mirror.queueRows.value)
        assertEquals(EntryStatus.PLAYING, mirror.queue.value.item("a")?.status)

        // The next patch is against the same base: the row the last one changed goes back.
        mirror.apply(
            StateSlice.QueuePatch(
                1uL,
                listOf(item("a", 1, status = EntryStatus.PLAYED), item("b", 2, status = EntryStatus.PLAYING)),
            ),
        )
        assertEquals(EntryStatus.PLAYED, mirror.queue.value.item("a")?.status)
        mirror.apply(StateSlice.QueuePatch(1uL, listOf(item("c", 3, status = EntryStatus.PLAYING))))
        assertEquals(EntryStatus.QUEUED, mirror.queue.value.item("a")?.status)
        assertEquals(EntryStatus.QUEUED, mirror.queue.value.item("b")?.status)
        assertEquals(EntryStatus.PLAYING, mirror.queue.value.item("c")?.status)
        assertEquals(EntryStatus.PLAYING, mirror.queueIndex.value.byTrack[3]?.status)
        assertEquals(EntryStatus.QUEUED, mirror.queueIndex.value.byTrack[1]?.status)
    }

    @Test
    fun `a patch for a queue sent whole since is stale and ignored`() = runTest {
        mirror.apply(StateSlice.Queue(listOf(item("a", 1)), 2uL))
        val before = mirror.queue.value
        val woken = emissions(mirror.queue) {
            mirror.apply(StateSlice.QueuePatch(1uL, listOf(item("a", 1, status = EntryStatus.PLAYING))))
        }
        assertEquals(0, woken)
        assertSame(before, mirror.queue.value)
    }

    @Test
    fun `an empty patch on an unpatched queue says nothing`() = runTest {
        mirror.apply(StateSlice.Queue(listOf(item("a", 1)), 1uL))
        assertEquals(0, emissions(mirror.queue) { mirror.apply(StateSlice.QueuePatch(1uL, emptyList())) })
    }

    @Test
    fun `a patch for a row the queue no longer holds is skipped`() {
        mirror.apply(StateSlice.Queue(listOf(item("a", 1)), 1uL))
        mirror.apply(StateSlice.QueuePatch(1uL, listOf(item("gone", 9, status = EntryStatus.PLAYING))))
        assertEquals(EntryStatus.QUEUED, mirror.queue.value.item("a")?.status)
    }

    @Test
    fun `transfers are structural and figures are keyed by track`() {
        mirror.apply(StateSlice.Figures(emptyList()))
        assertTrue(mirror.figures.value.isEmpty())
        assertEquals(0, mirror.activeTransfers)
    }

    @Test
    fun `follow applies each batch and ends with the engine`() = runTest {
        val stream = FakeStream()
        val job = launch(UnconfinedTestDispatcher(testScheduler)) { mirror.follow(stream) }
        stream.send(StateSlice.Library(1uL), StateSlice.Cache(5uL))
        assertEquals(1L, mirror.libraryVersion.value)
        assertEquals(5L, mirror.cacheBytes.value)
        assertEquals(1L, mirror.batches.value)
        stream.send(StateSlice.Library(2uL))
        assertEquals(2L, mirror.batches.value)
        assertFalse(job.isCompleted)
        stream.close()
        assertTrue(job.isCompleted)
    }

    @Test
    fun `waitUntil returns at once when settled, and times out when not`() = runTest {
        assertTrue(mirror.waitUntil(50) { true })
        val job = launch(UnconfinedTestDispatcher(testScheduler)) {
            assertFalse(mirror.waitUntil(50) { mirror.libraryVersion.value > 0 })
        }
        testScheduler.advanceTimeBy(100)
        job.join()
    }

    @Test
    fun `waitUntil wakes on the batch that settles it`() = runTest {
        val stream = FakeStream()
        launch(UnconfinedTestDispatcher(testScheduler)) { mirror.follow(stream) }
        var result: Boolean? = null
        launch(UnconfinedTestDispatcher(testScheduler)) {
            result = mirror.waitUntil(10_000) { mirror.libraryVersion.value >= 2 }
        }
        stream.send(StateSlice.Library(1uL))
        assertNull(result)
        stream.send(StateSlice.Library(2uL))
        assertEquals(true, result)
        stream.close()
    }

    @Test
    fun `devices carry when they arrived and which is controlled`() {
        mirror.apply(StateSlice.Devices(emptyList(), "phone"))
        assertEquals("phone", mirror.devices.value.target)
        assertEquals(now, mirror.devices.value.atNanos)
        assertNull(mirror.devices.value.controlled)
    }
}
