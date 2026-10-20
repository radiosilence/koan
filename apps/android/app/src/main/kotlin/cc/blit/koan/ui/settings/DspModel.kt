package cc.blit.koan.ui.settings

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import cc.blit.koan.engine.describe
import cc.blit.koan.ffi.AutoEqEntry
import cc.blit.koan.ffi.AutoEqMaker
import cc.blit.koan.ffi.AutoEqOffer
import cc.blit.koan.ffi.DeletedDsp
import cc.blit.koan.ffi.DspEarKind
import cc.blit.koan.ffi.DspImportPlan
import cc.blit.koan.ffi.DspImportSummary
import cc.blit.koan.ffi.DspLayerInfo
import cc.blit.koan.ffi.DspMeasurementReading
import cc.blit.koan.ffi.DspOverview
import cc.blit.koan.ffi.DspPoint
import cc.blit.koan.ffi.DspProfileDetail
import cc.blit.koan.ffi.DspProfileSummary
import cc.blit.koan.ffi.DspResponse
import cc.blit.koan.ffi.DspRole
import cc.blit.koan.ffi.DspSavedCopy
import cc.blit.koan.ffi.DspStarter
import cc.blit.koan.ffi.DspTargetName
import cc.blit.koan.ffi.DspTargetOption
import cc.blit.koan.ffi.DspTargets
import cc.blit.koan.ffi.DspTuningEntry
import cc.blit.koan.ffi.KoanEngineInterface
import cc.blit.koan.ffi.KoanException
import cc.blit.koan.ffi.SquigHit
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.serialization.Serializable

/** A stage of a device's chain, to choose what fills it. */
@Serializable
enum class Stage { Correction, Eq }

/** The stage of a device's chain an import was started from, which what it made goes into once its role is answered. */
data class DspPlacement(val device: String, val stage: Stage)

/** Files chosen together, and what importing them will do, waiting to be confirmed and named. */
data class PendingImport(val paths: List<String>, val plan: DspImportPlan, val rate: UInt?, val into: DspPlacement? = null)

/** An import that stopped for the rate of what it was given. */
sealed interface NeedsRate {
    data class Files(val paths: List<String>, val name: String?, val into: DspPlacement? = null) : NeedsRate

    data class Text(val text: String) : NeedsRate
}

/**
 * The profiles an import asks the role of, once for all of them, and where
 * the import was started. `profiles` is what goes into that chain: the group
 * several presets became, or the profiles made, the first as a correction.
 */
data class RoleAsk(val names: List<String>, val profiles: List<String> = names, val into: DspPlacement? = null)

data class DspState(
    val overview: DspOverview? = null,
    /** What the output in use plays, drawn: its correction and tuning together. */
    val output: DspResponse? = null,
    /** Moves on every change made here, for a profile's page to read again. */
    val version: Int = 0,
    val error: String? = null,
    /** What the last import of several files did. */
    val summary: String? = null,
    val pendingImport: PendingImport? = null,
    val needsRate: NeedsRate? = null,
    val askRole: RoleAsk? = null,
    /** AutoEQ's profile for the output in use, by its name, while it has none and the offer stands. */
    val suggestion: AutoEqOffer? = null,
    /** AutoEQ's results for the last search settled. */
    val autoEqResults: List<AutoEqEntry> = emptyList(),
    /** The makers in AutoEQ's index, to browse when nothing is typed. */
    val autoEqMakers: List<AutoEqMaker> = emptyList(),
)

/**
 * EQ and convolution profiles, and importing them: the counterpart of
 * `DspModel.swift`. One import flow wherever it starts, Settings' file
 * picker or a file shared or opened with the app: several files are planned
 * and confirmed first, coefficients that carry no rate ask for one, and each
 * new profile is asked what it is for.
 *
 * `overview` is the output in use, the phone's own route keyed as the core
 * keys it (`DspOverview.device`); the EQ page can show any device the EQ
 * names. Nothing plays through a profile until one is chosen for it.
 *
 * Scoped to the activity, so a share arriving on any page lands here.
 */
class DspModel(private val engine: suspend () -> KoanEngineInterface) : ViewModel() {
    private val _state = MutableStateFlow(DspState())
    val state: StateFlow<DspState> = _state.asStateFlow()

    fun reload() {
        viewModelScope.launch {
            changed(bump = false)
            val suggestion = engine().autoeqSuggestion()
            _state.update { it.copy(suggestion = suggestion) }
        }
    }

    // -- AutoEQ ------------------------------------------------------------

    /** Moves with every search, so one answered after a newer one began is dropped. */
    private var search = 0

    /** Search AutoEQ by headphone name. The page debounces; this runs once per settled query. */
    suspend fun searchAutoEq(query: String) {
        val mine = ++search
        val trimmed = query.trim()
        if (trimmed.isEmpty()) {
            _state.update { it.copy(autoEqResults = emptyList()) }
            return
        }
        try {
            val found = engine().autoeqSearch(trimmed, 40u)
            if (mine == search) _state.update { it.copy(autoEqResults = found) }
        } catch (e: KoanException) {
            if (mine == search) failed(e)
        }
    }

    /** The makers to browse, read once and kept: the index changes daily at most. */
    fun loadAutoEqMakers() {
        if (_state.value.autoEqMakers.isNotEmpty()) return
        viewModelScope.launch {
            try {
                val makers = engine().autoeqMakers()
                _state.update { it.copy(autoEqMakers = makers) }
            } catch (e: KoanException) {
                failed(e)
            }
        }
    }

    /** `maker`'s results, by model. */
    suspend fun autoEqModels(maker: String): List<AutoEqEntry> = try {
        engine().autoeqModels(maker)
    } catch (e: KoanException) {
        failed(e)
        emptyList()
    }

    /** Install `entry` as a profile and correct the output in use with it. */
    fun installAutoEq(entry: AutoEqEntry) {
        val device = _state.value.overview?.device
        _state.update { it.copy(suggestion = null) }
        act { autoeqInstall(entry.name, entry.measuredBy, device) }
    }

    /** Stop offering AutoEQ's profile for the output in use. */
    fun dismissSuggestion() {
        _state.update { it.copy(suggestion = null) }
        act { autoeqDismiss() }
    }

    // -- Importing ---------------------------------------------------------

    /**
     * Files, folders or zips, as paths the core can read. Several are first
     * planned and shown to be confirmed: whole presets become a group, parts
     * of one profile combine into one.
     */
    fun importPaths(paths: List<String>, name: String? = null, rate: UInt? = null, into: DspPlacement? = null) {
        if (paths.isEmpty()) return
        viewModelScope.launch {
            if (paths.size > 1 && name == null) {
                val plan = engine().dspImportPlan(paths)
                _state.update { it.copy(pendingImport = PendingImport(paths, plan, rate, into)) }
                return@launch
            }
            run(paths, name, rate, into)
        }
    }

    /** Import what `pendingImport` planned, under `name`. */
    fun confirmImport(name: String) {
        val pending = _state.value.pendingImport ?: return
        _state.update { it.copy(pendingImport = null) }
        viewModelScope.launch { run(pending.paths, name.trim(), pending.rate, pending.into) }
    }

    fun cancelImport() = _state.update { it.copy(pendingImport = null) }

    /** EQ written out as text: pasted, as Equalizer APO or AutoEQ's parametric files read. */
    fun importText(text: String, rate: UInt? = null) {
        if (text.isBlank()) return
        viewModelScope.launch {
            try {
                val name = engine().dspImportText(text, null, rate)
                _state.update { it.copy(error = null, askRole = RoleAsk(listOf(name))) }
            } catch (_: KoanException.NeedsSampleRate) {
                _state.update { it.copy(needsRate = NeedsRate.Text(text)) }
            } catch (e: KoanException) {
                failed(e)
            }
            changed()
        }
    }

    /** The import that stopped for a rate, with one. */
    fun retry(rate: UInt) {
        val pending = _state.value.needsRate ?: return
        _state.update { it.copy(needsRate = null) }
        when (pending) {
            // Already planned and confirmed: straight to the import.
            is NeedsRate.Files -> viewModelScope.launch { run(pending.paths, pending.name, rate, pending.into) }
            is NeedsRate.Text -> importText(pending.text, rate)
        }
    }

    fun cancelRate() = _state.update { it.copy(needsRate = null) }

    private suspend fun run(paths: List<String>, name: String?, rate: UInt?, into: DspPlacement?) {
        try {
            val summary = engine().dspImportFiles(paths, name, rate)
            _state.update {
                it.copy(
                    error = null,
                    // A group's members are alike, so one answer does for them all.
                    askRole = summary.imported.takeIf { names -> names.isNotEmpty() }?.let { names ->
                        RoleAsk(names, summary.group?.let(::listOf) ?: names, into)
                    },
                    summary = describe(summary, paths.size),
                )
            }
        } catch (_: KoanException.NeedsSampleRate) {
            _state.update { it.copy(needsRate = NeedsRate.Files(paths, name, into)) }
        } catch (e: KoanException) {
            _state.update { it.copy(summary = null) }
            failed(e)
        }
        changed()
    }

    /**
     * Say what the imported profiles are for, or with `null` leave them
     * tunings. One started from a device's chain then goes into it: a
     * correction as the device's correction, a tuning on the end of its
     * filters.
     */
    fun answer(ask: RoleAsk, role: DspRole?) {
        _state.update { it.copy(askRole = null) }
        if (role == null && ask.into == null) return
        act {
            if (role != null) ask.names.forEach { dspSetRole(it, role) }
            val into = ask.into ?: return@act
            if (role == DspRole.CORRECTION || role == DspRole.BAKED) {
                dspAssignDevice(into.device, ask.profiles.firstOrNull())
            } else {
                // A tuning goes on the end of the filters, whichever stage the import began from.
                val chain = dspOverviewFor(into.device).chain
                val added = ask.profiles.fold(chain) { c, name -> c.added(name) }
                if (added != chain) dspSetTunings(into.device, added)
            }
        }
    }

    // -- Choosing ----------------------------------------------------------

    /** Correct the output in use with `profile`, or with `null` play it untouched. */
    fun use(profile: String?) = act { dspAssign(profile) }

    /** Correct `device` with `profile`, or with `null` play it untouched, whether or not it is the output in use. */
    fun assign(profile: String?, device: String) = act { dspAssignDevice(device, profile) }

    /** The profiles, and what `device` plays: the output in use with `null`. */
    suspend fun overviewFor(device: String?) = engine().dspOverviewFor(device)

    /** What `device` plays, drawn: its correction and tuning. */
    suspend fun outputResponse(device: String) = engine().dspOutputResponseFor(device, 48_000u)

    /** Make `device`'s tuning these EQs, in the order they play. */
    fun setTunings(entries: List<DspTuningEntry>, device: String) = act { dspSetTunings(device, entries) }

    /** Set `device` from the preset `name`, or flat with `null`. */
    fun applyPreset(name: String?, device: String) = act { dspApplyPreset(device, name) }

    /** Set the output in use from the preset `name`, or flat with `null`. */
    fun applyPreset(name: String?) {
        val device = _state.value.overview?.device ?: return
        applyPreset(name, device)
    }

    /** Save `device`'s correction and tuning as the preset `name`, over one of that name. Whether it took. */
    fun savePreset(name: String, device: String, then: (Boolean) -> Unit = {}) {
        viewModelScope.launch {
            val ok = try {
                engine().dspSavePreset(device, name.trim())
                _state.update { it.copy(error = null) }
                true
            } catch (e: KoanException) {
                failed(e)
                false
            }
            changed()
            then(ok)
        }
    }

    /** Have `device` play its filters alone, its correction taken off, or correct for a headphone again. */
    fun setFiltersOnly(only: Boolean, device: String) = act { dspSetFiltersOnly(device, only) }

    /** Play `member` of the group `group`. */
    fun select(group: String, member: String) = act { dspSelect(group, member) }

    /** Make `name` a group, one member playing at a time, or a stack of layers played in order. */
    fun setGroup(name: String, group: Boolean) = act { dspSetGroup(name, group) }

    /** Make `name` a stack of `layers`, in order. */
    fun setLayers(name: String, layers: List<DspLayerInfo>) = act { dspSetLayers(name, layers) }

    // -- Targets -----------------------------------------------------------

    /** The targets `name`'s correction can be moved to. */
    suspend fun targets(name: String): DspTargets? = engine().dspTargets(name)

    /** Move `name`'s correction to `id`, or with `null` back to its own target. */
    fun chooseTarget(name: String, id: String?) = act { dspChooseTarget(name, id) }

    /** Add a target from a file the core can read. */
    fun addTarget(path: String) = act { dspAddTarget(path) }

    /** The target a ready-made correction was made for, or `null` for Unknown. */
    fun setMadeFor(name: String, target: String?) = act { dspSetMadeFor(name, target) }

    /** The target the tuning `name` was made against, or `null` for not known. */
    fun setTunedFor(name: String, target: String?) = act { dspSetTunedFor(name, target) }

    /** The targets for a kind of headphone or a speaker. */
    suspend fun targetsFor(ear: DspEarKind): List<DspTargetOption> = engine().dspTargetsFor(ear)

    /** What the tuning `name` looks made against on the output in use, where it does not say. */
    suspend fun suggestMadeAgainst(name: String): DspTargetName? = engine().dspSuggestMadeAgainst(name, null)

    /** What the tuning `name` adds on the output's correction for each target it could be made against, by id; "" for not saying. */
    suspend fun madeAgainstPreviews(name: String): Map<String, List<Double>> =
        engine().dspMadeAgainstPreviews(name, null).associate { (it.target ?: "") to it.db }

    // -- Measurements ------------------------------------------------------

    /** What `text` is read as, as a measurement; throws why it cannot be one. */
    suspend fun describeMeasurement(text: String): DspMeasurementReading = engine().dspDescribeMeasurement(text)

    /** What correcting a measurement to `target` would do, before saving. */
    suspend fun previewMeasurement(text: String, target: String): DspResponse = engine().dspPreviewMeasurement(text, target)

    /** Measurements on squig.link sites matching `query`, best first. */
    suspend fun squigSearch(query: String): List<SquigHit> = engine().dspSquigSearch(query)

    /** `hit`'s measurement, its channels averaged, as frequency and level. */
    suspend fun squigFetch(hit: SquigHit): String = engine().dspSquigFetch(hit.site, hit.file)

    /** Save a measurement corrected to `target` as a profile, crediting `source`. */
    suspend fun saveMeasured(name: String, text: String, ear: DspEarKind, target: String, source: String?): String {
        val saved = engine().dspSaveMeasured(name.trim(), text, ear, target, source)
        changed()
        return saved
    }

    /** What splitting the baked EQ `name` would give: the correction, the tuning, their sum, and the EQ itself. */
    suspend fun previewSplit(name: String, text: String, target: String): DspResponse = engine().dspPreviewSplit(name, text, target)

    /** Split the baked EQ `name` into a correction and a tuning; the outputs that played it play the two. */
    suspend fun splitBaked(name: String, text: String, inEar: Boolean, target: String): List<String> {
        val made = engine().dspSplitBaked(name, text, inEar, target)
        changed()
        return made
    }

    // -- Kept ---------------------------------------------------------------

    /** Keep `name` on every device of the account, or on this one alone. */
    fun setScope(name: String, everywhere: Boolean) = act { dspSetScope(name, everywhere) }

    /** EQs deleted in the last thirty days, which the server keeps; `null` where they could not be read. */
    suspend fun deleted(): List<DeletedDsp>? = try {
        engine().dspDeleted()
    } catch (_: KoanException) {
        null
    }

    /** Bring a deleted EQ back on every device; why not, where it would not come. */
    suspend fun restore(uid: String): String? = try {
        engine().dspRestore(uid)
        changed()
        null
    } catch (e: KoanException) {
        e.describe()
    }

    // -- A profile ---------------------------------------------------------

    suspend fun detail(name: String): DspProfileDetail? = engine().dspDetail(name)

    /** What `name` does to the sound, at 48 kHz, for the graph. */
    suspend fun response(name: String): DspResponse? = engine().dspResponse(name, 48_000u)

    fun setRole(name: String, role: DspRole) = act { dspSetRole(name, role) }

    /** Edits made here, by profile: a page that has seen none takes a change to its profile as another device's. */
    private val edits = mutableMapOf<String, Int>()

    fun edits(name: String) = edits[name] ?: 0

    private fun edited(name: String) {
        edits[name] = edits(name) + 1
    }

    /** `name`'s filters and headroom as they are, for `saveAsCopy` to put back. */
    suspend fun snapshot(name: String): String? = engine().dspSnapshot(name)

    /**
     * Keep `name` as it is now as a copy, and put `name` back to `before` or
     * its file; on `device` the copy takes its place in the tuning. The copy,
     * or `null` where it was refused.
     */
    suspend fun saveAsCopy(name: String, new: String?, before: String?, device: String?): DspSavedCopy? {
        val copy = try {
            val made = engine().dspSaveAsCopy(name, new?.trim()?.takeIf { it.isNotEmpty() }, before, device)
            _state.update { it.copy(error = null) }
            made
        } catch (e: KoanException) {
            failed(e)
            null
        }
        changed()
        return copy
    }

    /** Set band `index` of `name`; it plays at once. */
    fun setBand(name: String, index: Int, kind: String, freq: Double, gain: Double, q: Double) {
        edited(name)
        act { dspSetBand(name, index.toUInt(), kind, freq, gain, q) }
    }

    /** What `name` would draw with band `index` set so, nothing saved. */
    suspend fun previewBand(name: String, index: Int, kind: String, freq: Double, gain: Double, q: Double) =
        engine().dspPreviewBand(name, index.toUInt(), kind, freq, gain, q, 48_000u)

    /** Set the points of graphic curve `index` of `name`. */
    fun setCurve(name: String, index: Int, points: List<DspPoint>) {
        edited(name)
        act { dspSetCurve(name, index.toUInt(), points) }
    }

    /** What `name` would draw with curve `index` set to `points`, nothing saved. */
    suspend fun previewCurve(name: String, index: Int, points: List<DspPoint>) =
        engine().dspPreviewCurve(name, index.toUInt(), points, 48_000u)

    /** A flat band at 1 kHz, on `channel`, or on both channels. */
    fun addBand(name: String, channel: Int? = null) {
        edited(name)
        act { dspAddBand(name, channel?.toUShort()) }
    }

    /** A list of bands for each channel, or one for both from channel `keep`. */
    fun setStereo(name: String, stereo: Boolean, keep: Int = 0) {
        edited(name)
        act { dspSetStereo(name, stereo, keep.toUShort()) }
    }

    /**
     * Make the EQ `name`, eight bands from `starter`, and put it on the end of
     * `device`'s tuning, the output in use's with `null`. `then` has why it
     * was refused, or null once it is made and in place; the refusal is the
     * dialog's to show, not the page's.
     */
    fun createEq(name: String, starter: DspStarter, device: String? = null, then: (String?) -> Unit) {
        viewModelScope.launch {
            val engine = engine()
            val refusal = try {
                engine.dspCreateEq(name, starter)
                null
            } catch (e: KoanException) {
                e.describe()
            }
            if (refusal == null) {
                // Made, whatever the tuning says next: a failure there is the page's.
                try {
                    val o = engine.dspOverviewFor(device)
                    o.device?.let { engine.dspSetTunings(it, o.chain.added(name)) }
                    _state.update { it.copy(error = null) }
                } catch (e: KoanException) {
                    failed(e)
                }
            }
            changed()
            then(refusal)
        }
    }

    fun removeFilter(name: String, index: Int) {
        edited(name)
        act { dspRemoveFilter(name, index.toUInt()) }
    }

    /** Whether it took: a name already in use is refused. */
    fun rename(old: String, new: String, then: (Boolean) -> Unit = {}) {
        viewModelScope.launch {
            val ok = try {
                engine().dspRename(old, new.trim())
                // A page that follows the profile under its new name still knows its own edits.
                edits.remove(old)?.let { edits[new.trim()] = it }
                _state.update { it.copy(error = null) }
                true
            } catch (e: KoanException) {
                failed(e)
                false
            }
            changed()
            then(ok)
        }
    }

    /** A copy as it is now, as `new` or "<name> copy". */
    fun duplicate(name: String, new: String? = null) = act { dspDuplicate(name, new?.trim()?.takeIf { it.isNotEmpty() }) }

    /** Put `name` back as it was imported. */
    fun revert(name: String) = act { dspRevert(name) }

    /** Delete `name` everywhere it is kept; `then` once it is gone, or was refused. */
    fun delete(name: String, then: () -> Unit = {}) = act(then) { dspDelete(name) }

    fun dismissError() = _state.update { it.copy(error = null) }

    private fun act(then: () -> Unit = {}, work: suspend KoanEngineInterface.() -> Unit) {
        viewModelScope.launch {
            try {
                engine().work()
                _state.update { it.copy(error = null) }
            } catch (e: KoanException) {
                failed(e)
            }
            changed()
            then()
        }
    }

    private suspend fun changed(bump: Boolean = true) {
        val overview = engine().dspOverview()
        // The old curve stays until the new one is drawn, so the graph never
        // empties between changes.
        val output = overview.device?.let { engine().dspOutputResponseFor(it, 48_000u) } ?: _state.value.output
        _state.update { it.copy(overview = overview, output = output, version = if (bump) it.version + 1 else it.version) }
    }

    private fun failed(e: KoanException) = _state.update { it.copy(error = e.describe()) }

    companion object {
        /** Rates coefficients are commonly designed at. */
        val rates = listOf(44_100u, 48_000u, 88_200u, 96_000u, 176_400u, 192_000u)

        fun khz(hz: UInt): String {
            val k = hz.toDouble() / 1000
            return if (k == Math.rint(k)) "%.0f".format(k) else "%.1f".format(k)
        }

        /** What an import did, in a sentence or two: nothing to say for one file made into one profile. */
        fun describe(summary: DspImportSummary, files: Int): String? {
            val lines = mutableListOf<String>()
            val group = summary.group
            if (group != null) {
                val playing = summary.imported.firstOrNull()?.let { " “$it” is playing; pick another on the group's page." } ?: ""
                lines += "Imported ${summary.imported.size} presets as the group “$group”.$playing"
            } else if (files > 1 && summary.imported.isNotEmpty()) {
                lines += "Combined $files files into “${summary.imported.first()}”."
            }
            if (summary.refused.isNotEmpty()) {
                lines += "${summary.refused.size} refused: " + summary.refused.joinToString("; ") { "${it.file}: ${it.reason}" }
            }
            lines += summary.notes
            return lines.takeIf { it.isNotEmpty() }?.joinToString(" ")
        }

        /** What a profile is, in a line. */
        fun describe(p: DspProfileSummary): String {
            val parts = mutableListOf<String>()
            if (p.measured) parts += "From a measurement"
            when {
                p.members.isNotEmpty() -> parts += "${p.members.size} to pick from"
                p.preset -> parts += if (p.layers == 1u) "1 part" else "${p.layers} parts"
                p.layers > 0u -> parts += "Plays ${p.layers} ${if (p.layers == 1u) "EQ" else "EQs"} in order"
            }
            // A graphic curve is one filter of many points.
            if (p.graphics > 0u) parts += "graphic, ${p.points} ${if (p.points == 1u) "point" else "points"}"
            val bands = p.bands - p.graphics
            if (bands > 0u) parts += "$bands ${if (bands == 1u) "band" else "bands"}"
            if (p.rates.isNotEmpty()) parts += p.rates.joinToString(", ") { khz(it) } + " kHz"
            return parts.joinToString(" · ")
        }
    }
}

val DspRole.label
    get() = when (this) {
        DspRole.CORRECTION -> "Correction"
        DspRole.TUNING -> "Tuning"
        DspRole.BAKED -> "Correction + Tuning"
    }

/** What each role means, as the import's question puts it. */
val DspRole.example
    get() = when (this) {
        DspRole.CORRECTION ->
            "Makes headphones or speakers sound neutral: an AutoEQ result, or a correction from a measurement such as spinorama's or REW's."
        DspRole.BAKED -> "Corrects and adds taste in one, as presets named for a sound do, like Qudelix's “Lush”."
        DspRole.TUNING -> "Taste on top of a correction: more bass, a darker treble."
    }
