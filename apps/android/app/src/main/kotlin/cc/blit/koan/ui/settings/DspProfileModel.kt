@file:OptIn(ExperimentalCoroutinesApi::class)

package cc.blit.koan.ui.settings

import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import cc.blit.koan.ffi.DspEarKind
import cc.blit.koan.ffi.DspOverview
import cc.blit.koan.ffi.DspProfileDetail
import cc.blit.koan.ffi.DspResponse
import cc.blit.koan.ffi.DspRole
import cc.blit.koan.ffi.DspTargetName
import cc.blit.koan.ffi.DspTargets
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/** What a profile's page shows: the profile as read, and what it is compared with. */
data class DspProfileView(
    val name: String,
    val detail: DspProfileDetail? = null,
    val response: DspResponse? = null,
    val targets: DspTargets? = null,
    /** The device Used for acts on, as it is now. */
    val targetOverview: DspOverview? = null,
    val madeForChoices: TargetGroups = TargetGroups(),
    /** For a tuning: the target it looks made against while it does not say, and what it adds on the output's correction for each choice. */
    val suggestion: DspTargetName? = null,
    val previews: Map<String, List<Double>> = emptyMap(),
    /** The EQ as it was when the page opened, which Save as copy puts back, and as it is now. */
    val before: String? = null,
    val now: String? = null,
    /** What became of the last Save as copy, where it needs saying. */
    val notice: String? = null,
)

/**
 * One profile's page, read again whenever a change lands, here or synced: the
 * counterpart of `DspProfilePage`'s state on the Apple apps. `device` is the
 * output whose filters and correction Used for acts on, the one in use if
 * `null`. The name is kept in `saved`, so a renamed profile is still the one
 * shown after the process is recreated.
 */
class DspProfileModel(
    private val saved: SavedStateHandle,
    private val dsp: DspModel,
    library: Flow<Long>,
    initial: String,
    device: String?,
) : ViewModel() {
    private val name = saved.getStateFlow(NAME, initial)
    private val _state = MutableStateFlow(DspProfileView(name.value))
    val state: StateFlow<DspProfileView> = _state.asStateFlow()

    /** Edits made here when `before` was taken: until there are more, a change is another device's, and `before` follows it. */
    private var editsAtBefore: Int? = null

    /** The device Used for acts on. */
    private val target = dsp.state.map { device ?: it.overview?.device }.distinctUntilChanged()

    init {
        viewModelScope.launch {
            combine(name, dsp.state.map { it.version }.distinctUntilChanged(), library, target) { n, _, _, t -> n to t }
                .collectLatest { (n, t) -> read(n, t) }
        }
        // The targets there are, read again only when one is added or the output's ear kind changes.
        viewModelScope.launch {
            dsp.state.map { it.targetsVersion to it.overview?.inEar }.distinctUntilChanged().collectLatest { (_, inEar) ->
                coroutineScope {
                    val over = async { dsp.targetsFor(DspEarKind.OVER_EAR) }
                    val ear = async { dsp.targetsFor(DspEarKind.IN_EAR) }
                    val choices = TargetGroups.of(over.await(), ear.await(), inEar)
                    _state.update { it.copy(madeForChoices = choices) }
                }
            }
        }
    }

    private suspend fun read(name: String, target: String?) = coroutineScope {
        val snapshot = async { dsp.snapshot(name) }
        val d = async { dsp.detail(name) }
        val r = async { dsp.response(name) }
        val t = async { dsp.targets(name) }
        val o = async { target?.let { dsp.overviewFor(it) } }
        val now = snapshot.await()
        val detail = d.await()
        val tuning = detail?.role == DspRole.TUNING
        val s = async { if (tuning) dsp.suggestMadeAgainst(name) else null }
        val p = async { if (tuning) dsp.madeAgainstPreviews(name) else emptyMap() }
        val read = DspProfileView(name, detail, r.await(), t.await(), o.await(), suggestion = s.await(), previews = p.await(), now = now)
        // Decided as the read lands, not as it starts: a Save as copy or a
        // reset while it was out has cleared `before` for this read to take.
        val edits = dsp.edits(name)
        val current = _state.value
        val keep = current.before != null && editsAtBefore != edits
        if (!keep) editsAtBefore = edits
        _state.value = read.copy(
            madeForChoices = current.madeForChoices,
            notice = current.notice,
            before = if (keep) current.before else now,
        )
    }

    /** Rename the profile; `then` hears whether it took. */
    fun rename(new: String, then: (Boolean) -> Unit) {
        dsp.rename(name.value, new) { ok ->
            if (ok) saved[NAME] = new.trim()
            then(ok)
        }
    }

    /** Keep the profile as it is now as `new`, put it back as it was when the page opened, and show the copy. */
    fun saveAsCopy(new: String, device: String?, label: (String) -> String) {
        val (from, kept) = name.value to _state.value.before
        viewModelScope.launch {
            val copy = dsp.saveAsCopy(from, new, kept, device) ?: return@launch
            val notice = if (copy.placed) null
            else "${copy.name} is saved, but $from is not in ${device?.let(label) ?: "the output"}'s filters, so the copy is not either. Add it on the EQ page."
            _state.update { it.copy(notice = notice, before = null) }
            saved[NAME] = copy.name
        }
    }

    /** Put the profile back as its file had it; what the page opened with is taken again. */
    fun revert() {
        dsp.revert(name.value)
        _state.update { it.copy(before = null) }
    }

    private companion object {
        const val NAME = "name"
    }
}
