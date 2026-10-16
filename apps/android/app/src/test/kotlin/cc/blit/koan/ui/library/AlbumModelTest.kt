package cc.blit.koan.ui.library

import cc.blit.koan.ffi.AlbumPage
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Before
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class AlbumModelTest {
    private val dispatcher = StandardTestDispatcher()
    private val library = MutableStateFlow(0L)
    private val gone = AlbumPage(null, emptyList())

    @Before fun setUp() = Dispatchers.setMain(dispatcher)

    @After fun tearDown() = Dispatchers.resetMain()

    @Test
    fun aFailedReadSaysSoAndTryAgainReadsAgain() = runTest(dispatcher) {
        var failing = true
        val model = AlbumModel({ if (failing) error("database is locked") else gone }, library)
        backgroundScope.launch { model.page.collect {} }
        assertEquals(AlbumRead.Reading, model.page.value)
        advanceUntilIdle()
        assertEquals(AlbumRead.Failed, model.page.value)
        failing = false
        model.retry()
        advanceUntilIdle()
        assertEquals(AlbumRead.Read(gone), model.page.value)
    }

    @Test
    fun theLibraryMovingReadsAgain() = runTest(dispatcher) {
        var reads = 0
        val model = AlbumModel({ reads++; gone }, library)
        backgroundScope.launch { model.page.collect {} }
        advanceUntilIdle()
        library.value = 1
        advanceUntilIdle()
        assertEquals(2, reads)
    }
}
