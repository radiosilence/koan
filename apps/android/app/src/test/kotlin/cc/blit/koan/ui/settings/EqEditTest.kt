package cc.blit.koan.ui.settings

import cc.blit.koan.ffi.AutoEqEntry
import cc.blit.koan.ffi.AutoEqOffer
import cc.blit.koan.ffi.DspBand
import cc.blit.koan.ffi.DspOverview
import cc.blit.koan.ffi.DspPoint
import cc.blit.koan.ffi.DspProfileSummary
import cc.blit.koan.ffi.DspRole
import cc.blit.koan.ffi.DspStarter
import cc.blit.koan.ffi.DspTuningEntry
import cc.blit.koan.ffi.KoanException
import kotlin.math.abs
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/** Editing EQ: bands and curves, the tuning chain, presets, and AutoEQ, against a stand-in engine. */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(application = android.app.Application::class, sdk = [35])
class EqEditTest {
    private val dispatcher = StandardTestDispatcher()
    private val fake = FakeEngine()

    @Before fun setUp() {
        Dispatchers.setMain(dispatcher)
        fake.answers["dspOverview"] = { overview() }
        fake.answers["dspOverviewFor"] = { overview() }
        fake.answers["dspOutputResponseFor"] = { null }
        fake.answers["autoeqSuggestion"] = { null }
    }

    @After fun tearDown() = Dispatchers.resetMain()

    private fun band(kind: String, freq: Double = 1000.0, gain: Double = 0.0, channels: List<UShort> = emptyList(), curve: List<DspPoint> = emptyList()) =
        DspBand(kind, freq, gain, 1.0, channels, 0.0, 0.0, emptyList(), curve)

    private fun overview(chain: List<DspTuningEntry> = emptyList()) = DspOverview(
        enabled = true, device = "speaker", active = null, tuning = null, tuningPlays = false, leftOut = null,
        leftOutEqs = emptyList(), outputs = emptyList(), tunings = emptyMap(), chain = chain, joins = emptyList(), aim = null,
        inEar = null, preset = null, presetEdited = false, profiles = emptyList(), names = emptyMap(), filtersOnly = false,
    )

    @Test fun handlesAreThePeaksAndShelvesOnTheChannelDrawn() {
        val bands = listOf(band("peaking"), band("low_pass"), band("high_shelf", channels = listOf(1u)), band("low_shelf"))
        assertEquals(listOf(0, 3), handles(bands).map { it.index })
        assertEquals(listOf(0, 2, 3), handles(bands, channel = 1).map { it.index })
        assertTrue(bands[2].on(1) && !bands[2].on(0) && bands[0].on(0) && bands[0].on(1))
        assertTrue(editable("notch"))
        assertTrue(!editable("graphic"))
    }

    @Test fun theCurvePaintedIsTheFirstOnTheLeft() {
        assertEquals(2, curveIndex(listOf(band("peaking"), band("graphic", channels = listOf(1u)), band("graphic"))))
        assertNull(curveIndex(listOf(band("peaking"))))
    }

    @Test fun aPointBrushMovesTheNearestPointAlone() {
        val points = listOf(DspPoint(100.0, 0.0), DspPoint(1000.0, 0.0), DspPoint(10_000.0, 0.0))
        assertEquals(listOf(0.0, 6.0, 0.0), paint(points, 1100.0, 6.0, Brush.Point).map { it.db })
    }

    @Test fun aWiderBrushPullsNeighboursLessTheFurtherTheyAre() {
        val points = listOf(1000.0, 1260.0, 2000.0, 8000.0).map { DspPoint(it, 0.0) }
        val narrow = paint(points, 1000.0, 6.0, Brush.Narrow).map { it.db }
        assertEquals(6.0, narrow[0], 0.0)
        assertTrue(narrow[1] in 0.1..6.0)
        assertEquals(0.0, narrow[2], 0.0)
        val wide = paint(points, 1000.0, 6.0, Brush.Wide).map { it.db }
        assertTrue(wide[2] > 0 && wide[2] < wide[1])
        // Kept to a hundredth of a dB.
        assertTrue(wide.all { abs(it * 100 - Math.rint(it * 100)) < 1e-9 })
    }

    @Test fun theDevicesAreTheOneInUseThenTheOneShownThenTheEqs() {
        fun profile(name: String, devices: List<String>, preset: Boolean = false) = DspProfileSummary(
            name = name, devices = devices, bands = 10u, layers = 0u, rates = emptyList(), problem = null, members = emptyList(),
            playing = null, role = DspRole.CORRECTION, measured = false, preset = preset, edited = false, usedOn = emptyList(),
            everywhere = false, heldBy = emptyList(), scopeLocked = null, graphics = 0u, points = 0u, madeFor = null, join = null,
        )
        val o = overview().copy(
            profiles = listOf(profile("HD 600", listOf("dac", "speaker")), profile("Evening", listOf("tv"), preset = true)),
            tunings = mapOf("car" to "Warm"),
        )
        assertEquals(listOf("speaker", "usb", "dac", "car"), devices(o, inUse = "speaker", shown = "usb"))
    }

    @Test fun aChainIsReorderedSwitchedAndTrimmed() {
        val chain = listOf(DspTuningEntry("Warm", true), DspTuningEntry("Bright", true), DspTuningEntry("Loud", false))
        assertEquals(listOf("Bright", "Warm", "Loud"), chain.moved(1, -1).map { it.name })
        assertEquals(chain, chain.moved(0, -1))
        assertEquals(listOf(true, false, false), chain.toggled(1).map { it.on })
        assertEquals(listOf("Warm", "Loud"), chain.removed(1).map { it.name })
        assertEquals(chain, chain.added("Warm"))
        assertEquals(DspTuningEntry("Vocal", true), chain.added("Vocal").last())
    }

    @Test fun aTypedFigureCommitsOnlyWhenItChanges() {
        assertEquals(1500.0, figure("1500", "1000", 0)!!, 0.0)
        assertNull(figure("1000", "1000", 0))
        assertNull(figure("loud", "1000", 0))
        assertEquals(2.5, figure(" 2,5 ", "0.0", 1)!!, 0.0)
    }

    @Test fun aDragSavesAtMostEveryQuarterSecondAndAtItsEnd() = runTest(dispatcher) {
        var clock = 0L
        val shown = mutableListOf<Int>()
        val saves = mutableListOf<Int>()
        val stepper = EditStepper<Int>(this, { clock }) { shown += it }
        for (step in 1..5) {
            stepper.step(done = false, preview = { step }) { saves += step }
            advanceUntilIdle()
            clock += 100
        }
        stepper.step(done = true, preview = { 6 }) { saves += 6 }
        advanceUntilIdle()
        // The first step waits; then one in each quarter second; then the end.
        assertEquals(listOf(4, 6), saves)
        assertEquals(listOf(1, 2, 3, 4, 5), shown)
    }

    @Test fun aSlowPreviewDrawsOnlyTheNewestNext() = runTest(dispatcher) {
        val slow = CompletableDeferred<Int>()
        val shown = mutableListOf<Int>()
        val stepper = EditStepper<Int>(this, { 0L }) { shown += it }
        stepper.step(false, { slow.await() }) {}
        runCurrent()
        stepper.step(false, { 2 }) {}
        stepper.step(false, { 3 }) {}
        slow.complete(1)
        advanceUntilIdle()
        assertEquals(listOf(1, 3), shown)
    }

    @Test fun leavingSavesWhatTheThrottleHeldBack() = runTest(dispatcher) {
        val saves = mutableListOf<Int>()
        val stepper = EditStepper<Int>(this, { 0L }) {}
        stepper.step(false, { 1 }) { saves += 1 }
        stepper.flush()
        assertEquals(listOf(1), saves)
        assertTrue(!stepper.editing)
    }

    @Test fun bandsAndCurvesAreSetByIndex() = runTest(dispatcher) {
        val model = DspModel { fake.engine }
        model.setBand("Warm", 2, "peaking", 120.0, 3.0, 0.7)
        model.setCurve("Warm", 0, listOf(DspPoint(100.0, 1.0)))
        fake.answers["dspAddBand"] = { 3u }
        model.addBand("Warm")
        model.removeFilter("Warm", 1)
        advanceUntilIdle()
        // An unsigned argument reaches the engine as its bits: a plain Int.
        assertEquals(listOf(listOf<Any?>("Warm", 2, "peaking", 120.0, 3.0, 0.7)), fake.called("dspSetBand"))
        assertEquals(listOf(listOf<Any?>("Warm", 0, listOf(DspPoint(100.0, 1.0)))), fake.called("dspSetCurve"))
        assertEquals(listOf(listOf<Any?>("Warm", null)), fake.called("dspAddBand"))
        assertEquals(listOf(listOf<Any?>("Warm", 1)), fake.called("dspRemoveFilter"))
    }

    @Test fun aNewEqGoesOnTheEndOfTheTuningAndARefusalStaysInTheDialog() = runTest(dispatcher) {
        val model = DspModel { fake.engine }
        model.reload()
        advanceUntilIdle()
        var made: String? = "not yet"
        model.createEq("Evening", DspStarter.BASS_BOOST) { made = it }
        advanceUntilIdle()
        assertNull(made)
        assertEquals(listOf(listOf<Any?>("Evening", DspStarter.BASS_BOOST)), fake.called("dspCreateEq"))
        assertEquals(listOf(listOf<Any?>("speaker", listOf(DspTuningEntry("Evening", true)))), fake.called("dspSetTunings"))

        fake.answers["dspCreateEq"] = { throw KoanException.BadArgument("There is already an EQ called Evening") }
        var refused: String? = null
        model.createEq("Evening", DspStarter.FLAT) { refused = it }
        advanceUntilIdle()
        assertTrue(refused.orEmpty().contains("already"))
        assertNull(model.state.value.error)

        // Made, but the tuning would not take it: not a refusal of the name.
        fake.answers.remove("dspCreateEq")
        fake.answers["dspSetTunings"] = { throw KoanException.Database("locked") }
        var night: String? = "not yet"
        model.createEq("Night", DspStarter.FLAT) { night = it }
        advanceUntilIdle()
        assertNull(night)
        assertTrue(model.state.value.error != null)
    }

    @Test fun stereoKeepsTheChannelAsked() = runTest(dispatcher) {
        val model = DspModel { fake.engine }
        model.setStereo("Warm", true)
        model.setStereo("Warm", false, keep = 1)
        model.addBand("Warm", channel = 1)
        advanceUntilIdle()
        // A u16 reaches the engine as its bits, a Short; an optional one stays boxed, a UShort.
        assertEquals(listOf(listOf<Any?>("Warm", true, 0.toShort()), listOf<Any?>("Warm", false, 1.toShort())), fake.called("dspSetStereo"))
        assertEquals(listOf(listOf<Any?>("Warm", 1.toUShort())), fake.called("dspAddBand"))
    }

    @Test fun theChainAndPresetsAreTheOutputsOwn() = runTest(dispatcher) {
        fake.answers["dspSavePreset"] = { "Evening" }
        val model = DspModel { fake.engine }
        model.reload()
        advanceUntilIdle()
        val chain = listOf(DspTuningEntry("Warm", true))
        model.setTunings(chain, "speaker")
        model.applyPreset(null, "speaker")
        var saved: Boolean? = null
        model.savePreset(" Evening ", "speaker") { saved = it }
        advanceUntilIdle()
        assertEquals(listOf(listOf<Any?>("speaker", chain)), fake.called("dspSetTunings"))
        assertEquals(listOf(listOf<Any?>("speaker", null)), fake.called("dspApplyPreset"))
        assertEquals(listOf(listOf<Any?>("speaker", "Evening")), fake.called("dspSavePreset"))
        assertEquals(true, saved)
        assertEquals("Flat", presetSummary(overview()))
        assertEquals("Unsaved", presetSummary(overview(chain)))
    }

    @Test fun anAutoEqResultIsInstalledForTheOutputInUse() = runTest(dispatcher) {
        val entry = AutoEqEntry("Sennheiser HD 600", "oratory1990", "Sennheiser HD 600 (oratory1990)")
        fake.answers["autoeqSuggestion"] = { AutoEqOffer.Profile(entry) }
        val model = DspModel { fake.engine }
        model.reload()
        advanceUntilIdle()
        assertEquals(AutoEqOffer.Profile(entry), model.state.value.suggestion)
        fake.answers["autoeqInstall"] = { entry.profileName }
        model.installAutoEq(entry)
        advanceUntilIdle()
        assertEquals(listOf(listOf<Any?>("Sennheiser HD 600", "oratory1990", "speaker")), fake.called("autoeqInstall"))
        assertNull(model.state.value.suggestion)
    }

    @Test fun anOfferTurnedDownIsNotMadeAgain() = runTest(dispatcher) {
        fake.answers["autoeqSuggestion"] = { AutoEqOffer.Search("HD 6") }
        val model = DspModel { fake.engine }
        model.reload()
        advanceUntilIdle()
        model.dismissSuggestion()
        advanceUntilIdle()
        assertNull(model.state.value.suggestion)
        assertEquals(1, fake.called("autoeqDismiss").size)
    }

    @Test fun aSearchFindsAndAnEmptyOneClears() = runTest(dispatcher) {
        val hd = AutoEqEntry("HD 600", "a", "HD 600 (a)")
        fake.answers["autoeqSearch"] = { (query) -> if (query == "hd") listOf(hd) else emptyList<AutoEqEntry>() }
        val model = DspModel { fake.engine }
        model.searchAutoEq(" hd ")
        assertEquals(listOf(hd), model.state.value.autoEqResults)
        assertEquals(listOf(listOf<Any?>("hd", 40)), fake.called("autoeqSearch"))
        model.searchAutoEq("  ")
        assertEquals(emptyList<AutoEqEntry>(), model.state.value.autoEqResults)
    }
}
