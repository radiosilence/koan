package cc.blit.koan.ui.settings

import android.app.Application
import androidx.compose.foundation.gestures.scrollBy
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Text
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.test.captureToImage
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.unit.dp
import cc.blit.koan.ffi.DspLayerCurve
import cc.blit.koan.ffi.DspOverview
import cc.blit.koan.ffi.DspResponse
import cc.blit.koan.ffi.DspRole
import cc.blit.koan.ffi.DspTuningEntry
import kotlin.math.abs
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/** The response graph: its scale and frame, what the output's graph draws, and that its paths are built once. */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(application = Application::class, sdk = [35])
class EqGraphTest {
    @get:Rule
    val compose = createComposeRule()

    private val style = GraphStyle(Color.Gray, Color.Black, Color.Gray, TextStyle.Default)
    private val freqs = (0..479).map { 20.0 * Math.pow(2.0, it / 48.0) }

    @Test
    fun thePathsAreBuiltOncePerSetOfCurves() {
        var builds = 0
        var unrelated by mutableIntStateOf(0)
        var gain by mutableStateOf(3.0)
        compose.setContent {
            Column {
                Text("$unrelated")
                val curves = remember(gain) { listOf(Curve(freqs.map { if (it in 900.0..1100.0) gain else 0.0 }, Color.Red)) }
                ResponseGraph(freqs, curves, style, Modifier.width(320.dp).height(200.dp), onBuild = { builds++ })
            }
        }
        // Each capture draws a frame.
        val frame = { compose.onRoot().captureToImage() }
        frame()
        assertEquals(1, builds)
        repeat(3) {
            unrelated++
            frame()
        }
        assertEquals("recomposing and drawing around it builds nothing", 1, builds)
        gain = 6.0
        frame()
        assertEquals("new data builds again", 2, builds)
    }

    @Test
    fun scrollingPastTheGraphBuildsNothing() {
        var builds = 0
        val list = LazyListState()
        compose.setContent {
            val curves = remember { listOf(Curve(freqs.map { 3.0 }, Color.Red)) }
            LazyColumn(Modifier.width(320.dp).height(400.dp), state = list) {
                items(2) { Text("Row $it", Modifier.height(80.dp)) }
                item { ResponseGraph(freqs, curves, style, Modifier.width(320.dp).height(200.dp), onBuild = { builds++ }) }
                items(20) { Text("Row ${it + 2}", Modifier.height(80.dp)) }
            }
        }
        val frame = { compose.onRoot().captureToImage() }
        frame()
        assertEquals(1, builds)
        for (by in listOf(40f, 40f, 40f, -40f)) {
            compose.runOnIdle { kotlinx.coroutines.runBlocking { list.scrollBy(by) } }
            frame()
        }
        assertEquals("each frame copies the graph it drew", 1, builds)
    }

    @Test
    fun aTuningCorrectingIsOfferedAsATuning() {
        assertEquals(PhoneAction.MoveToTuning, phoneAction("Warm", DspRole.TUNING, active = "Warm", inChain = false))
        assertEquals(PhoneAction.StopCorrecting, phoneAction("HD 650", DspRole.CORRECTION, active = "HD 650", inChain = false))
        assertEquals(PhoneAction.TakeOutOfTuning, phoneAction("Warm", DspRole.TUNING, active = null, inChain = true))
        assertEquals(PhoneAction.AddToTuning, phoneAction("Warm", DspRole.TUNING, active = null, inChain = false))
        assertEquals(PhoneAction.Correct, phoneAction("HD 650", DspRole.BAKED, active = null, inChain = false))
    }

    @Test
    fun aCurveIsDrawnWithAFixedNumberOfPointsLogSpaced() {
        val db = freqs.map { 20 * kotlin.math.log10(it / 1000) / 3 }
        val points = sample(freqs, db)
        assertEquals(POINTS, points.size)
        assertEquals(20.0, points.first().first, 1e-9)
        assertEquals(20_000.0, points.last().first, 1e-6)
        // Read off the curve, not off its nearest point.
        val (hz, level) = points[POINTS / 2]
        assertEquals(20 * kotlin.math.log10(hz / 1000) / 3, level, 0.01)
        assertTrue(sample(emptyList(), emptyList()).isEmpty())
    }

    @Test
    fun theScaleAndItsGridAreThePhonesAndMacs() {
        assertEquals(6.0, halfRange(listOf(listOf(1.0, -2.5))), 0.0)
        assertEquals(12.0, halfRange(listOf(listOf(7.0))), 0.0)
        assertEquals(listOf(-6.0, 0.0, 6.0), dbTicks(6.0))
        assertEquals(listOf(-20.0, -10.0, 0.0, 10.0, 20.0), dbTicks(24.0))
        assertEquals(listOf("20", "50", "100", "200", "500", "1k", "2k", "5k", "10k", "20k"), hzTicks.map(::hzLabel))
        val plot = Plot(1000f, 200f, 12.0, left = 30f, bottom = 16f)
        assertEquals(30f, plot.x(20.0), 0.01f)
        assertEquals(1000f, plot.x(20_000.0), 0.01f)
        assertEquals(92f, plot.y(0.0), 0.01f)
        for (hz in listOf(20.0, 100.0, 1000.0, 20_000.0)) assertEquals(hz, plot.hz(plot.x(hz)), hz * 1e-4)
        for (db in listOf(-12.0, 0.0, 6.0)) assertEquals(db, plot.db(plot.y(db)), 1e-4)
    }

    @Test
    fun theOutputsGraphDrawsEachStagePlayingThenTheWhole() {
        val total = freqs.map { 1.0 }
        val r = DspResponse(
            freqs = freqs, total = total, bands = emptyList(),
            layers = listOf(DspLayerCurve("Warm", true, freqs.map { 0.5 }), DspLayerCurve("Off", false, freqs.map { 9.0 })),
            measurement = null, target = null, predicted = null, preampDb = 0.0, gainDb = 0.0,
            correction = freqs.map { 0.5 }, tuning = null, original = null, right = null, rightGainDb = 0.0,
        )
        val o = overview(active = "HD 600", chain = listOf(DspTuningEntry("Warm", true), DspTuningEntry("Off", false)))
        val curves = outputCurves(o, r, Color.Black, Color.Red, Color.Gray)
        assertEquals(listOf(Color.Red, Color.Gray, Color.Black), curves.map { it.color })
        assertTrue(curves[1].dashed)
        assertTrue(abs(curves.last().db.first() - 1.0) < 1e-9)
    }

    private fun overview(active: String?, chain: List<DspTuningEntry>) = DspOverview(
        enabled = true, device = "speaker", active = active, tuning = null, tuningPlays = chain.isNotEmpty(), leftOut = null,
        leftOutEqs = emptyList(), outputs = emptyList(), tunings = emptyMap(), chain = chain, joins = emptyList(), aim = null,
        inEar = null, preset = null, presetEdited = false, profiles = emptyList(), names = emptyMap(), filtersOnly = false,
    )
}
