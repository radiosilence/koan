package cc.blit.koan.ui.search

import cc.blit.koan.ffi.ShelfSummary
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Before
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class SearchModelTest {
    private val dispatcher = StandardTestDispatcher()
    private val library = MutableStateFlow(0L)
    private val asked = mutableListOf<String>()

    /** Each answer's artist total is its query's length, so a result says which query it answers. */
    private fun answer(query: String) = ShelfSummary(emptyList(), query.length.toULong(), emptyList(), 0u, emptyList(), 0u)

    private fun model(slow: (String) -> Long = { 0 }) = SearchModel({ query ->
        asked += query
        delay(slow(query))
        answer(query)
    }, library)

    /** `results` is shared while collected, as the page does. */
    private fun TestScope.follow(model: SearchModel) = backgroundScope.launch { model.results.collect {} }

    @Before fun setUp() = Dispatchers.setMain(dispatcher)

    @After fun tearDown() = Dispatchers.resetMain()

    @Test fun typingAsksOnceForWhatWasTyped() = runTest(dispatcher) {
        val model = model()
        follow(model)
        for (prefix in listOf("b", "bo", "boa", "boar", "board")) {
            model.setQuery(prefix)
            advanceTimeBy(SearchModel.DEBOUNCE_MS / 2)
        }
        advanceUntilIdle()
        assertEquals(listOf("board"), asked)
        assertEquals("board", model.results.value?.query)
    }

    @Test fun aSlowAnswerNeverLandsOverANewerQuery() = runTest(dispatcher) {
        val model = model(slow = { if (it == "slow") 10_000 else 0 })
        follow(model)
        model.setQuery("slow")
        advanceTimeBy(SearchModel.DEBOUNCE_MS + 100)
        model.setQuery("fast")
        advanceUntilIdle()
        assertEquals(listOf("slow", "fast"), asked)
        assertEquals("fast", model.results.value?.query)
        assertEquals(4uL, model.results.value?.shelf?.artistTotal)
    }

    @Test fun clearingEmptiesAtOnce() = runTest(dispatcher) {
        val model = model()
        follow(model)
        model.setQuery("boards")
        advanceUntilIdle()
        model.setQuery("  ")
        advanceTimeBy(1)
        assertNull(model.results.value)
    }

    @Test fun theLibraryMovingAsksAgain() = runTest(dispatcher) {
        val model = model()
        follow(model)
        model.setQuery("boards")
        advanceUntilIdle()
        library.value = 1
        advanceUntilIdle()
        assertEquals(listOf("boards", "boards"), asked)
    }
}
