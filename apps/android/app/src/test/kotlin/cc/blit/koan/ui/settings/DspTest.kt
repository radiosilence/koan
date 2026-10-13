package cc.blit.koan.ui.settings

import android.content.Intent
import android.net.Uri
import cc.blit.koan.ffi.DspImportPlan
import cc.blit.koan.ffi.DspImportRefusal
import cc.blit.koan.ffi.DspImportSummary
import cc.blit.koan.ffi.DspOverview
import cc.blit.koan.ffi.DspProfileSummary
import cc.blit.koan.ffi.DspRole
import cc.blit.koan.ffi.DspTuningEntry
import cc.blit.koan.ffi.KoanException
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.resetMain
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
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config

/** The EQ import flow and choices against a stand-in engine, and the files the system hands over. */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(application = android.app.Application::class, sdk = [35])
class DspTest {
    private val dispatcher = StandardTestDispatcher()
    private val fake = FakeEngine()
    private val context = RuntimeEnvironment.getApplication()

    @Before fun setUp() {
        Dispatchers.setMain(dispatcher)
        fake.answers["dspOverview"] = { overview() }
    }

    @After fun tearDown() = Dispatchers.resetMain()

    private fun model() = DspModel { fake.engine }

    private fun imported(vararg names: String, group: String? = null) =
        DspImportSummary(names.toList(), emptyList(), emptyList(), group)

    @Test fun oneFileImportsAtOnceAndAsksWhatItIs() = runTest(dispatcher) {
        fake.answers["dspImportFiles"] = { imported("HD 600") }
        val model = model()
        model.importPaths(listOf("/cache/HD 600.txt"))
        advanceUntilIdle()
        assertEquals(listOf(listOf<Any?>(listOf("/cache/HD 600.txt"), null, null)), fake.called("dspImportFiles"))
        assertEquals(RoleAsk(listOf("HD 600")), model.state.value.askRole)
        assertTrue(fake.called("dspImportPlan").isEmpty())
    }

    @Test fun severalFilesArePlannedAndConfirmedFirst() = runTest(dispatcher) {
        val plan = DspImportPlan(group = true, files = listOf("Lush.txt", "Bright.txt"), name = "Qudelix")
        fake.answers["dspImportPlan"] = { plan }
        fake.answers["dspImportFiles"] = { imported("Lush", "Bright", group = "Mine") }
        val model = model()
        val paths = listOf("/cache/Lush.txt", "/cache/Bright.txt")
        model.importPaths(paths)
        advanceUntilIdle()
        assertEquals(PendingImport(paths, plan, null), model.state.value.pendingImport)
        assertTrue(fake.called("dspImportFiles").isEmpty())

        model.confirmImport(" Mine ")
        advanceUntilIdle()
        assertNull(model.state.value.pendingImport)
        assertEquals(listOf(listOf<Any?>(paths, "Mine", null)), fake.called("dspImportFiles"))
        assertEquals(RoleAsk(listOf("Lush", "Bright")), model.state.value.askRole)
        assertTrue(model.state.value.summary!!.startsWith("Imported 2 presets as the group “Mine”."))
    }

    @Test fun coefficientsWithoutARateAskForOne() = runTest(dispatcher) {
        var asked = 0
        fake.answers["dspImportFiles"] = { (_, _, rate) ->
            asked++
            if (rate == null) throw KoanException.NeedsSampleRate("no rate") else imported("Biquads")
        }
        val model = model()
        model.importPaths(listOf("/cache/biquads.txt"))
        advanceUntilIdle()
        assertEquals(NeedsRate.Files(listOf("/cache/biquads.txt"), null), model.state.value.needsRate)

        model.retry(48_000u)
        advanceUntilIdle()
        assertNull(model.state.value.needsRate)
        assertEquals(48_000u, fake.called("dspImportFiles").last()[2])
        assertEquals(2, asked)
    }

    @Test fun pastedTextIsImportedAsOne() = runTest(dispatcher) {
        fake.answers["dspImportText"] = { "Pasted" }
        val model = model()
        model.importText("Preamp: -6 dB\nFilter 1: ON PK Fc 100 Hz Gain 3 dB Q 1")
        advanceUntilIdle()
        assertEquals(RoleAsk(listOf("Pasted")), model.state.value.askRole)
    }

    @Test fun answeringSetsEachRoleAndDecidingLaterSetsNone() = runTest(dispatcher) {
        val model = model()
        model.answer(RoleAsk(listOf("A", "B")), DspRole.CORRECTION)
        advanceUntilIdle()
        assertEquals(listOf(listOf<Any?>("A", DspRole.CORRECTION), listOf<Any?>("B", DspRole.CORRECTION)), fake.called("dspSetRole"))
        assertNull(model.state.value.askRole)

        model.answer(RoleAsk(listOf("C")), null)
        advanceUntilIdle()
        assertEquals(2, fake.called("dspSetRole").size)
    }

    @Test fun bitPerfectIsChoosingNothing() = runTest(dispatcher) {
        val model = model()
        model.use(null)
        advanceUntilIdle()
        assertEquals(listOf(listOf<Any?>(null)), fake.called("dspAssign"))
        assertEquals("Phone speaker is flat: the music plays untouched.", sentence(overview()))
        assertEquals(
            "Music to Phone speaker, corrected by HD 600 to Harman, then through Warm.",
            sentence(overview(active = "HD 600", aim = "Harman", chain = listOf(DspTuningEntry("Warm", true)))),
        )
    }

    @Test fun aRefusedImportSaysWhy() = runTest(dispatcher) {
        fake.answers["dspImportFiles"] = {
            DspImportSummary(emptyList(), listOf(DspImportRefusal("notes.txt", "not an EQ")), emptyList(), null)
        }
        val model = model()
        model.importPaths(listOf("/cache/notes.txt"))
        advanceUntilIdle()
        assertNull(model.state.value.askRole)
        assertEquals("1 refused: notes.txt: not an EQ", model.state.value.summary)
    }

    @Test fun aProfileIsDescribedInALine() {
        assertEquals("From a measurement · 10 bands · 44.1, 48 kHz", DspModel.describe(profile(measured = true, bands = 10u, rates = listOf(44_100u, 48_000u))))
        assertEquals("3 to pick from", DspModel.describe(profile(members = listOf("a", "b", "c"))))
    }

    @Test fun sharedAndOpenedFilesAreTaken() {
        val one = Uri.parse("content://downloads/1")
        val two = Uri.parse("content://downloads/2")
        assertEquals(listOf(one), sharedFiles(Intent(Intent.ACTION_SEND).putExtra(Intent.EXTRA_STREAM, one)))
        assertEquals(
            listOf(one, two),
            sharedFiles(Intent(Intent.ACTION_SEND_MULTIPLE).putParcelableArrayListExtra(Intent.EXTRA_STREAM, arrayListOf(one, two))),
        )
        assertEquals(listOf(one), sharedFiles(Intent(Intent.ACTION_VIEW, one)))
        // A link is the account's, not a file.
        assertTrue(sharedFiles(Intent(Intent.ACTION_VIEW, Uri.parse("koan://join?x=1"))).isEmpty())
        assertTrue(sharedFiles(Intent(Intent.ACTION_SEND).putExtra(Intent.EXTRA_TEXT, "hello")).isEmpty())
    }

    @Test fun aFileIsCopiedInUnderItsOwnName() = runTest(dispatcher) {
        val source = File(context.cacheDir, "HD 600 ParametricEQ.txt").apply { writeText("Preamp: -6 dB") }
        val copied = copyIn(context, listOf(Uri.fromFile(source)))
        assertEquals("HD 600 ParametricEQ.txt", File(copied.single()).name)
        assertEquals("Preamp: -6 dB", File(copied.single()).readText())
        assertEquals("evil", safeName("../../evil"))
        assertNull(safeName(".."))
    }

    @Test fun eqFilesSharedOrOpenedReachTheApp() {
        fun resolves(intent: Intent) = context.packageManager.queryIntentActivities(intent.setPackage(context.packageName), 0)
            .any { it.activityInfo.name == "cc.blit.koan.MainActivity" }
        val file = Uri.parse("content://downloads/1")
        assertTrue(resolves(Intent(Intent.ACTION_SEND).setType("application/zip")))
        assertTrue(resolves(Intent(Intent.ACTION_SEND_MULTIPLE).setType("text/plain")))
        assertTrue(resolves(Intent(Intent.ACTION_VIEW).setDataAndType(file, "audio/wav")))
        assertTrue(resolves(Intent(Intent.ACTION_VIEW).setDataAndType(file, "text/plain")))
        assertTrue(!resolves(Intent(Intent.ACTION_SEND).setType("image/png")))
    }

    private fun overview(active: String? = null, aim: String? = null, chain: List<DspTuningEntry> = emptyList()) = DspOverview(
        enabled = true, device = "speaker", active = active, tuning = null, tuningPlays = chain.isNotEmpty(),
        leftOut = null, leftOutEqs = emptyList(), outputs = emptyList(), tunings = emptyMap(), chain = chain,
        joins = emptyList(), aim = aim, inEar = null, preset = null, presetEdited = false, profiles = emptyList(),
        names = mapOf("speaker" to "Phone speaker"), filtersOnly = false,
    )

    private fun profile(measured: Boolean = false, bands: UInt = 0u, rates: List<UInt> = emptyList(), members: List<String> = emptyList()) =
        DspProfileSummary(
            name = "p", devices = emptyList(), bands = bands, layers = 0u, rates = rates, problem = null, members = members,
            playing = null, role = DspRole.CORRECTION, measured = measured, preset = false, edited = false, usedOn = emptyList(),
            everywhere = false, heldBy = emptyList(), scopeLocked = null, graphics = 0u, points = 0u, madeFor = null, join = null,
        )
}
