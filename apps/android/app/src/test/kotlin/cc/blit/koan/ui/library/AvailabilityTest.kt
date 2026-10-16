package cc.blit.koan.ui.library

import cc.blit.koan.ffi.TransferState
import org.junit.Assert.assertEquals
import org.junit.Test

class AvailabilityTest {
    @Test
    fun theLibraryRowSaysWhereTheBytesAreWhenNothingIsFetching() {
        assertEquals(Availability.Kept, availability(onServer = true, onDisk = true, transfer = null))
        assertEquals(Availability.ServerOnly, availability(onServer = true, onDisk = false, transfer = null))
        assertEquals(Availability.Local, availability(onServer = false, onDisk = true, transfer = null))
        assertEquals(Availability.None, availability(onServer = false, onDisk = false, transfer = null))
    }

    @Test
    fun aTransferSpeaksOverTheLibraryRow() {
        assertEquals(Availability.Queued, availability(true, false, TransferState.QUEUED))
        assertEquals(Availability.Downloading, availability(true, false, TransferState.RUNNING))
        assertEquals(Availability.Failed, availability(true, false, TransferState.FAILED))
        // Landed before the library has been read again.
        assertEquals(Availability.Kept, availability(true, false, TransferState.DONE))
    }
}
