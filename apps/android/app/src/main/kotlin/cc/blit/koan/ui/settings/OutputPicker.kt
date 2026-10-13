package cc.blit.koan.ui.settings

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.ffi.KoanEngineInterface
import cc.blit.koan.ffi.KoanException
import cc.blit.koan.ffi.OutputChoice
import cc.blit.koan.ffi.OutputInfo
import cc.blit.koan.ffi.OutputsInfo
import cc.blit.koan.ffi.RendererOutput
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/** Where this phone's music comes out, as the Output half of Play on shows it. */
sealed interface OutputState {
    /** This phone's own output. */
    data object Here : OutputState

    /** A renderer picked, its session opening: a few round trips to it. */
    data class Connecting(val udn: String, val name: String) : OutputState

    /** Played to a renderer; `problem` is why its last track was skipped, until one plays. */
    data class Playing(val udn: String, val name: String, val problem: String?) : OutputState

    /** A renderer that would not take the music. The music stays where it was. */
    data class Failed(val udn: String, val name: String, val reason: String) : OutputState
}

/**
 * What the output is: a pick still opening, one refused, the renderer the
 * engine plays to, or this phone.
 */
/** The renderer the engine plays to now, if any: what the output menu has ticked. */
fun playingTo(outputs: OutputsInfo?, renderer: RendererOutput?): String? =
    (outputs?.current as? OutputChoice.Renderer)?.udn ?: renderer?.udn

fun outputState(outputs: OutputsInfo?, renderer: RendererOutput?, connecting: OutputInfo?, failed: Pair<OutputInfo, String>?): OutputState {
    connecting?.let { return OutputState.Connecting(it.id, it.name) }
    failed?.let { (r, why) -> return OutputState.Failed(r.id, r.name, why) }
    val udn = playingTo(outputs, renderer) ?: return OutputState.Here
    val name = outputs?.renderers?.firstOrNull { it.id == udn }?.name ?: renderer?.name ?: udn
    return OutputState.Playing(udn, name, renderer?.takeIf { it.udn == udn }?.problem)
}

data class OutputPickerState(
    val connecting: OutputInfo? = null,
    val failed: Pair<OutputInfo, String>? = null,
    /** The search the sheet opened with is still waiting for answers. */
    val searching: Boolean = false,
)

/**
 * The Output half of Play on: UPnP renderers on the network, and this phone's
 * own output, as iOS's `OutputPicker` lists them. Opening the sheet searches,
 * which also holds the Wi-Fi multicast lock for the search's window; while it
 * stays open it searches again now and then, and closing it searches no more.
 */
class OutputPicker(private val engine: suspend () -> KoanEngineInterface, private val scope: CoroutineScope) {
    private val _state = MutableStateFlow(OutputPickerState())
    val state: StateFlow<OutputPickerState> = _state.asStateFlow()

    /**
     * Search as the sheet opens, saying so until its answers are in, then
     * again every `RESEARCH_MS` until the caller is cancelled.
     */
    suspend fun searchWhileOpen() {
        _state.update { it.copy(searching = true) }
        try {
            engine().searchRenderers()
            delay(SEARCH_MS)
            _state.update { it.copy(searching = false) }
            while (true) {
                delay(RESEARCH_MS)
                engine().searchRenderers()
            }
        } finally {
            _state.update { it.copy(searching = false) }
        }
    }

    /** Play to `renderer`, or back on this phone with `null`. */
    fun choose(renderer: OutputInfo?) {
        _state.update { it.copy(connecting = renderer, failed = null) }
        scope.launch {
            try {
                engine().selectOutput(renderer?.let { OutputChoice.Renderer(it.id) } ?: OutputChoice.Default)
                _state.update { it.copy(connecting = null) }
            } catch (e: KoanException) {
                _state.update { it.copy(connecting = null, failed = renderer?.let { r -> r to e.describe() }) }
            }
        }
    }

    fun dismissFailure() = _state.update { it.copy(failed = null) }

    companion object {
        /** How long a search waits for answers: the multicast lock's window. */
        const val SEARCH_MS = 5_000L

        /** How often the sheet, while open, searches again for renderers turned on meanwhile. */
        const val RESEARCH_MS = 15_000L
    }
}

/** What the renderers' list says when it has none. */
internal fun noRenderers(searching: Boolean, blocked: Boolean) = when {
    blocked -> "Allow nearby devices in Settings to find speakers and streamers on this network."
    searching -> "Looking for speakers and streamers on this network…"
    else -> "No UPnP speakers or streamers found on this network. Turn one on, and it appears here."
}

/**
 * The Output half of the sheet: this phone's own output, then the renderers
 * the engine has found, each saying how it is going.
 */
@Composable
fun OutputSection(picker: OutputPicker, outputs: OutputsInfo?, renderer: RendererOutput?, blocked: Boolean) {
    val state by picker.state.collectAsStateWithLifecycle()
    val output = outputState(outputs, renderer, state.connecting, state.failed)
    // What is ticked is what plays, whatever is opening or was refused.
    val playing = playingTo(outputs, renderer)
    val renderers = outputs?.renderers.orEmpty()
    Column {
        KoanText("Output", inset.padding(top = KoanSpace.l), role = KoanType.TitleSmall, tone = KoanTone.Strong, appText = true)
        OutputRow(
            name = outputs?.devices?.firstOrNull()?.name?.let { "This phone · $it" } ?: "This phone",
            detail = playing?.let { udn -> "Playing on ${renderers.firstOrNull { it.id == udn }?.name ?: renderer?.name ?: "a speaker"}. Pick this to bring the music back." },
            selected = playing == null,
            onSelect = { picker.choose(null) },
        )
        renderers.forEach { r ->
            OutputRow(
                name = r.name,
                detail = rendererDetail(r, output),
                selected = playing == r.id,
                warning = when (output) {
                    is OutputState.Failed -> output.udn == r.id
                    is OutputState.Playing -> output.udn == r.id && output.problem != null
                    else -> false
                },
                onSelect = { picker.choose(r) },
            )
        }
        if (renderers.isEmpty()) {
            KoanText(noRenderers(state.searching, blocked), inset.padding(vertical = KoanSpace.s), role = KoanType.Fine, tone = KoanTone.Muted)
        }
    }
}

/** A renderer's line: how picking it is going, else what it is. */
internal fun rendererDetail(r: OutputInfo, output: OutputState): String {
    val what = listOf(r.detail.ifEmpty { null }, "UPnP").filterNotNull().joinToString(" · ")
    return when {
        output is OutputState.Connecting && output.udn == r.id -> "Connecting…"
        output is OutputState.Failed && output.udn == r.id -> "Couldn't play here: ${output.reason}"
        output is OutputState.Playing && output.udn == r.id -> output.problem ?: "Playing here · $what"
        r.busy -> "In use by something else. Picking it takes over."
        else -> what
    }
}

@Composable
private fun OutputRow(name: String, detail: String?, selected: Boolean, onSelect: () -> Unit, warning: Boolean = false) {
    KoanButton(onSelect, inset.fillMaxWidth(), kind = KoanButtonKind.Card) {
        Row(Modifier.fillMaxWidth().padding(vertical = KoanSpace.s), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.width(28.dp)) {
                if (selected) KoanIcon(KoanIcons.check, contentDescription = "Playing through", color = KoanTheme.color(KoanTone.Accent))
            }
            Column(Modifier.weight(1f)) {
                KoanText(name, tone = if (selected) KoanTone.Accent else KoanTone.Ink)
                detail?.let { KoanText(it, role = KoanType.Fine, tone = if (warning) KoanTone.Bad else KoanTone.Muted) }
            }
        }
    }
}
