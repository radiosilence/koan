package cc.blit.koan.ui.settings

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import cc.blit.koan.ffi.AutoEqEntry
import cc.blit.koan.ffi.AutoEqMaker
import cc.blit.koan.ffi.AutoEqOffer
import cc.blit.koan.ffi.DspImportPlan
import cc.blit.koan.ffi.DspImportSummary
import cc.blit.koan.ffi.DspOverview
import cc.blit.koan.ffi.DspPoint
import cc.blit.koan.ffi.DspProfileDetail
import cc.blit.koan.ffi.DspProfileSummary
import cc.blit.koan.ffi.DspResponse
import cc.blit.koan.ffi.DspRole
import cc.blit.koan.ffi.DspTuningEntry
import cc.blit.koan.ffi.KoanEngineInterface
import cc.blit.koan.ffi.KoanException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/** Files chosen together, and what importing them will do, waiting to be confirmed and named. */
data class PendingImport(val paths: List<String>, val plan: DspImportPlan, val rate: UInt?)

/** An import that stopped for the rate of what it was given. */
sealed interface NeedsRate {
    data class Files(val paths: List<String>, val name: String?) : NeedsRate

    data class Text(val text: String) : NeedsRate
}

/** The profiles an import asks the role of: once for all of them. */
data class RoleAsk(val names: List<String>)

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
 * The output is the phone's own route, keyed as the core keys it
 * (`DspOverview.device`). Nothing plays through a profile until one is
 * chosen for it.
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
    fun importPaths(paths: List<String>, name: String? = null, rate: UInt? = null) {
        if (paths.isEmpty()) return
        viewModelScope.launch {
            if (paths.size > 1 && name == null) {
                val plan = engine().dspImportPlan(paths)
                _state.update { it.copy(pendingImport = PendingImport(paths, plan, rate)) }
                return@launch
            }
            run(paths, name, rate)
        }
    }

    /** Import what `pendingImport` planned, under `name`. */
    fun confirmImport(name: String) {
        val pending = _state.value.pendingImport ?: return
        _state.update { it.copy(pendingImport = null) }
        viewModelScope.launch { run(pending.paths, name.trim(), pending.rate) }
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
            is NeedsRate.Files -> viewModelScope.launch { run(pending.paths, pending.name, rate) }
            is NeedsRate.Text -> importText(pending.text, rate)
        }
    }

    fun cancelRate() = _state.update { it.copy(needsRate = null) }

    private suspend fun run(paths: List<String>, name: String?, rate: UInt?) {
        try {
            val summary = engine().dspImportFiles(paths, name, rate)
            _state.update {
                it.copy(
                    error = null,
                    // A group's members are alike, so one answer does for them all.
                    askRole = summary.imported.takeIf { names -> names.isNotEmpty() }?.let(::RoleAsk),
                    summary = describe(summary, paths.size),
                )
            }
        } catch (_: KoanException.NeedsSampleRate) {
            _state.update { it.copy(needsRate = NeedsRate.Files(paths, name)) }
        } catch (e: KoanException) {
            _state.update { it.copy(summary = null) }
            failed(e)
        }
        changed()
    }

    /** Say what the imported profiles are for, or with `null` leave them tunings. */
    fun answer(ask: RoleAsk, role: DspRole?) {
        _state.update { it.copy(askRole = null) }
        if (role == null) return
        act { ask.names.forEach { dspSetRole(it, role) } }
    }

    // -- Choosing ----------------------------------------------------------

    /** Correct the output in use with `profile`, or with `null` play it untouched. */
    fun use(profile: String?) = act { dspAssign(profile) }

    /** Make the output's tuning these EQs, in the order they play. */
    fun setTunings(entries: List<DspTuningEntry>) {
        val device = _state.value.overview?.device ?: return
        act { dspSetTunings(device, entries) }
    }

    /** Set the output from the preset `name`, or flat with `null`. */
    fun applyPreset(name: String?) {
        val device = _state.value.overview?.device ?: return
        act { dspApplyPreset(device, name) }
    }

    /** Save the output's correction and tuning as the preset `name`, over one of that name. Whether it took. */
    fun savePreset(name: String, then: (Boolean) -> Unit = {}) {
        val device = _state.value.overview?.device ?: return
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

    /** Play `member` of the group `group`. */
    fun select(group: String, member: String) = act { dspSelect(group, member) }

    // -- A profile ---------------------------------------------------------

    suspend fun detail(name: String): DspProfileDetail? = engine().dspDetail(name)

    /** What `name` does to the sound, at 48 kHz, for the graph. */
    suspend fun response(name: String): DspResponse? = engine().dspResponse(name, 48_000u)

    fun setRole(name: String, role: DspRole) = act { dspSetRole(name, role) }

    /** Set band `index` of `name`; it plays at once. */
    fun setBand(name: String, index: Int, kind: String, freq: Double, gain: Double, q: Double) =
        act { dspSetBand(name, index.toUInt(), kind, freq, gain, q) }

    /** What `name` would draw with band `index` set so, nothing saved. */
    suspend fun previewBand(name: String, index: Int, kind: String, freq: Double, gain: Double, q: Double) =
        engine().dspPreviewBand(name, index.toUInt(), kind, freq, gain, q, 48_000u)

    /** Set the points of graphic curve `index` of `name`. */
    fun setCurve(name: String, index: Int, points: List<DspPoint>) = act { dspSetCurve(name, index.toUInt(), points) }

    /** What `name` would draw with curve `index` set to `points`, nothing saved. */
    suspend fun previewCurve(name: String, index: Int, points: List<DspPoint>) =
        engine().dspPreviewCurve(name, index.toUInt(), points, 48_000u)

    /** A flat band at 1 kHz, on both channels. */
    fun addBand(name: String) = act { dspAddBand(name, null) }

    fun removeFilter(name: String, index: Int) = act { dspRemoveFilter(name, index.toUInt()) }

    /** Whether it took: a name already in use is refused. */
    fun rename(old: String, new: String, then: (Boolean) -> Unit = {}) {
        viewModelScope.launch {
            val ok = try {
                engine().dspRename(old, new.trim())
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

    fun delete(name: String) = act { dspDelete(name) }

    fun dismissError() = _state.update { it.copy(error = null) }

    private fun act(work: suspend KoanEngineInterface.() -> Unit) {
        viewModelScope.launch {
            try {
                engine().work()
                _state.update { it.copy(error = null) }
            } catch (e: KoanException) {
                failed(e)
            }
            changed()
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
