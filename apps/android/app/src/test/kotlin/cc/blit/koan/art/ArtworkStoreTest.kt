package cc.blit.koan.art

import cc.blit.koan.theme.KoanAccent
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.io.IOException

@OptIn(ExperimentalCoroutinesApi::class)
class ArtworkStoreTest {
    private class Covers(var answer: (ArtworkSource) -> ArtworkBytes? = { ArtworkBytes(byteArrayOf(1, 2, 3), "image/png") }) : CoverBytes {
        val asked = mutableListOf<Pair<ArtworkSource, UInt>>()
        var gate: CompletableDeferred<Unit>? = null
        override suspend fun fetch(source: ArtworkSource, pixels: UInt): ArtworkBytes? {
            asked += source to pixels
            gate?.await()
            return answer(source)
        }
    }

    private fun TestScope.store(covers: CoverBytes, colourOf: (ByteArray) -> Int? = { null }, max: Int = 512): ArtworkStore {
        val dispatcher = StandardTestDispatcher(testScheduler)
        return ArtworkStore(covers, colourOf, dispatcher, backgroundScope, max)
    }

    @Test
    fun `forty tiles of one record are one fetch`() = runTest {
        val covers = Covers().apply { gate = CompletableDeferred() }
        val store = store(covers)
        val tiles = List(40) { async { store.bytes(ArtworkSource.Album(1)) } }
        runCurrent()
        covers.gate!!.complete(Unit)
        runCurrent()
        val results = tiles.map { it.await() }
        assertEquals(1, covers.asked.size)
        assertTrue(results.all { it === results.first() && it != null })
    }

    @Test
    fun `the engine is asked for the one size that serves every draw`() = runTest {
        val covers = Covers()
        store(covers).bytes(ArtworkSource.Track(9))
        assertEquals(listOf<Pair<ArtworkSource, UInt>>(ArtworkSource.Track(9) to 1000u), covers.asked)
    }

    @Test
    fun `a record found to have no art is not asked about again until the library moves`() = runTest {
        val covers = Covers { null }
        val store = store(covers)
        assertNull(store.bytes(ArtworkSource.Album(1)))
        assertTrue(store.isAbsent(ArtworkSource.Album(1)))
        assertNull(store.bytes(ArtworkSource.Album(1)))
        assertEquals(1, covers.asked.size)

        covers.answer = { ArtworkBytes(byteArrayOf(9), "image/jpeg") }
        store.libraryChanged()
        assertFalse(store.isAbsent(ArtworkSource.Album(1)))
        assertNotNull(store.bytes(ArtworkSource.Album(1)))
        assertEquals(2, covers.asked.size)
    }

    @Test
    fun `a lookup that failed is not remembered as no art`() = runTest {
        var failing = true
        val covers = Covers { if (failing) throw IOException("offline") else ArtworkBytes(byteArrayOf(1), "image/png") }
        val store = store(covers)
        try {
            store.bytes(ArtworkSource.Album(1))
            fail("the failure should reach the caller")
        } catch (e: IOException) {
            assertEquals("offline", e.message)
        }
        assertFalse(store.isAbsent(ArtworkSource.Album(1)))
        failing = false
        assertNotNull(store.bytes(ArtworkSource.Album(1)))
    }

    @Test
    fun `warming starts the fetch and swallows a failure`() = runTest {
        val covers = Covers { throw IOException("offline") }
        val store = store(covers)
        store.warm(ArtworkSource.Album(4))
        runCurrent()
        assertEquals(1, covers.asked.size)
    }

    @Test
    fun `a record's colour is worked out once, from the bytes the sleeve needed`() = runTest {
        var decodes = 0
        val covers = Covers()
        val store = store(covers, colourOf = { decodes++; 0xCC3333 })
        val first = store.accent(ArtworkSource.Album(1))
        val second = store.accent(ArtworkSource.Album(1))
        assertEquals(first, second)
        assertEquals(KoanAccent.of(0xCC3333), first)
        assertEquals(1, decodes)
        assertEquals(1, covers.asked.size)
    }

    @Test
    fun `no art and no colour in the art are mint, and so is a lookup that failed`() = runTest {
        assertEquals(KoanAccent.Mint, store(Covers { null }).accent(ArtworkSource.Album(1)))
        assertEquals(KoanAccent.Mint, store(Covers(), colourOf = { null }).accent(ArtworkSource.Album(1)))
        assertEquals(KoanAccent.Mint, store(Covers { throw IOException("offline") }).accent(ArtworkSource.Album(1)))
    }

    @Test
    fun `held colours are bounded`() = runTest {
        val covers = Covers()
        val store = store(covers, colourOf = { 0x3366CC }, max = 2)
        for (id in 1L..3L) store.accent(ArtworkSource.Album(id))
        store.accent(ArtworkSource.Album(1)) // evicted: worked out again, from the bytes
        assertEquals("one fetch per record, and the evicted one again", 4, covers.asked.size)
    }

    @Test
    fun `a draw is read at the smallest bucket that is not smaller than it`() {
        assertEquals(64, ArtworkStore.bucket(40))
        assertEquals(64, ArtworkStore.bucket(64))
        assertEquals(128, ArtworkStore.bucket(65))
        assertEquals(256, ArtworkStore.bucket(200))
        assertEquals(512, ArtworkStore.bucket(512))
        assertEquals(1000, ArtworkStore.bucket(513))
        assertEquals(1000, ArtworkStore.bucket(4000))
    }

    @Test
    fun `a record's key is the same whichever size or tile asks`() {
        assertEquals("album:5", ArtworkSource.Album(5).key)
        assertEquals("track:5", ArtworkSource.Track(5).key)
        assertEquals("artist:5", ArtworkSource.Artist(5).key)
        assertEquals(ArtworkSource.Album(5), ArtworkSource.Album(5))
    }

    @Test
    fun `the bitmap cache is an eighth of the heap budget, between sixteen and ninety-six megabytes`() {
        val mb = 1024L * 1024
        assertEquals(16 * mb, artMemoryBytes(64))
        assertEquals(32 * mb, artMemoryBytes(256))
        assertEquals(64 * mb, artMemoryBytes(512))
        assertEquals(96 * mb, artMemoryBytes(2048))
    }
}
