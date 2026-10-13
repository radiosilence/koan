package cc.blit.koan.ui.settings

import cc.blit.koan.ffi.KoanException
import cc.blit.koan.ffi.OutputChoice
import cc.blit.koan.ffi.OutputInfo
import cc.blit.koan.ffi.OutputsInfo
import cc.blit.koan.ffi.RendererOutput
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/** The Output half of Play on: searching for renderers, picking one, and what each state says. */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(application = android.app.Application::class, sdk = [35])
class OutputPickerTest {
    private val fake = FakeEngine()
    private val amp = OutputInfo("uuid:amp", "Living room amp", "upnp", "WiiM Pro", false, null, false)
    private val tv = OutputInfo("uuid:tv", "Television", "upnp", "", true, null, false)
    private val phone = OutputInfo("Speaker", "Speaker", "builtin", "", false, null, false)

    private fun outputs(current: OutputChoice, vararg renderers: OutputInfo) =
        OutputsInfo(null, listOf(phone), renderers.toList(), current, null, emptyList(), false)

    @Test fun openingTheSheetSearchesOnceAndSaysSoUntilTheAnswersAreIn() = runTest {
        val picker = OutputPicker({ fake.engine }, backgroundScope)
        backgroundScope.launch { picker.search() }
        runCurrent()
        assertEquals(1, fake.called("searchRenderers").size)
        assertTrue(picker.state.value.searching)
        assertEquals("Looking for speakers and streamers on this network…", noRenderers(searching = true))

        advanceTimeBy(OutputPicker.SEARCH_MS + 1)
        assertFalse(picker.state.value.searching)
        advanceTimeBy(60_000)
        assertEquals("no timer: the list follows announcements", 1, fake.called("searchRenderers").size)

        backgroundScope.launch { picker.search() }
        runCurrent()
        assertEquals("Search again searches", 2, fake.called("searchRenderers").size)
    }

    @Test fun pickingARendererConnectsThenPlaysThere() = runTest {
        val gate = CompletableDeferred<Unit>()
        fake.answers["selectOutput"] = { Unit }
        val picker = OutputPicker({ gate.await(); fake.engine }, this)
        picker.choose(amp)
        assertEquals(OutputState.Connecting("uuid:amp", "Living room amp"), outputState(outputs(OutputChoice.Default, amp), null, picker.state.value.connecting, picker.state.value.failed))
        assertEquals("Connecting…", rendererDetail(amp, OutputState.Connecting(amp.id, amp.name)))
        gate.complete(Unit)
        advanceUntilIdle()
        assertEquals(listOf(listOf<Any?>(OutputChoice.Renderer("uuid:amp"))), fake.called("selectOutput"))
        assertNull(picker.state.value.connecting)
        val playing = outputState(outputs(OutputChoice.Renderer("uuid:amp"), amp), RendererOutput("uuid:amp", "Living room amp", null, null), null, null)
        assertEquals(OutputState.Playing("uuid:amp", "Living room amp", null), playing)
        assertEquals("Playing here · WiiM Pro · UPnP", rendererDetail(amp, playing))
    }

    @Test fun aRendererThatRefusesSaysWhyAndTheMusicStaysPut() = runTest {
        fake.answers["selectOutput"] = { throw KoanException.Audio("no answer from uuid:amp") }
        val picker = OutputPicker({ fake.engine }, this)
        picker.choose(amp)
        advanceUntilIdle()
        val failed = outputState(outputs(OutputChoice.Default, amp), null, picker.state.value.connecting, picker.state.value.failed)
        assertEquals(OutputState.Failed("uuid:amp", "Living room amp", "no answer from uuid:amp"), failed)
        assertEquals("Couldn't play here: no answer from uuid:amp", rendererDetail(amp, failed))
        // Still here: nothing was switched.
        assertNull(playingTo(outputs(OutputChoice.Default, amp), null))
    }

    @Test fun thisPhoneIsTheWayBack() = runTest {
        fake.answers["selectOutput"] = { Unit }
        val picker = OutputPicker({ fake.engine }, this)
        picker.choose(null)
        advanceUntilIdle()
        assertEquals(listOf(listOf<Any?>(OutputChoice.Default)), fake.called("selectOutput"))
        assertEquals(OutputState.Here, outputState(outputs(OutputChoice.Default, amp), null, null, null))
    }

    @Test fun eachRendererSaysWhatItIs() {
        val here = OutputState.Here
        assertEquals("WiiM Pro · UPnP", rendererDetail(amp, here))
        assertEquals("In use by something else. Picking it takes over.", rendererDetail(tv, here))
        val skipped = OutputState.Playing("uuid:amp", "Living room amp", "The amp could not play FLAC at 192 kHz")
        assertEquals("The amp could not play FLAC at 192 kHz", rendererDetail(amp, skipped))
        assertEquals("No UPnP speakers or streamers found on this network. Turn one on, and it appears here.", noRenderers(false))
        assertEquals("Allow nearby devices in Settings to find kōan devices on this network.", LOCAL_NETWORK_BLOCKED)
    }
}
