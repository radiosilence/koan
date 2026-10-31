package cc.blit.koan.engine

import kotlinx.coroutines.CancellationException
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class AttemptTest {
    @Test fun aFailureIsKept() {
        val result = attempt { error("database is locked") }
        assertTrue(result.isFailure)
        assertEquals("database is locked", result.exceptionOrNull()?.message)
    }

    @Test fun cancellationIsNotCaught() {
        assertThrows(CancellationException::class.java) { attempt { throw CancellationException("stopped") } }
    }

    @Test fun anAnswerIsKept() {
        assertEquals(3, attempt { 1 + 2 }.getOrNull())
    }
}
