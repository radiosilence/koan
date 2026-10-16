package cc.blit.koan.ui.settings

import android.content.Intent
import android.net.Uri
import android.provider.Settings as SystemSettings
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.engine.describe
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
    /** Why the controlled device did not take the output picked. */
    val refused: String? = null,
)

/**
 * The Output half of Play on: UPnP renderers on the network, and this phone's
 * own output, as iOS's `OutputPicker` lists them. Opening the sheet searches,
 * which also holds the Wi-Fi multicast lock for the search's window and lets
 * it go by itself; there is nothing to stop when the sheet closes.
 */
class OutputPicker(private val engine: suspend () -> KoanEngineInterface, private val scope: CoroutineScope) {
    private val _state = MutableStateFlow(OutputPickerState())
    val state: StateFlow<OutputPickerState> = _state.asStateFlow()

    /**
     * Search, saying so until the answers are in: they are unicast, all back
     * within the search's window. From then on the list follows renderers'
     * announcements by itself, so the sheet searches once as it opens, and
     * again only when asked.
     */
    suspend fun search() {
        _state.update { it.copy(searching = true) }
        try {
            engine().searchRenderers()
            delay(SEARCH_MS)
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

    /**
     * Play the controlled device through `choice`: the same command iOS sends
     * it. What it switched to comes back in its outputs; a refusal is shown.
     */
    fun chooseThere(choice: OutputChoice) {
        _state.update { it.copy(refused = null) }
        scope.launch {
            try {
                engine().selectOutput(choice)
            } catch (e: KoanException) {
                _state.update { it.copy(refused = e.describe()) }
            }
        }
    }

    /** The controlled device lists its outputs again, as it has them now. */
    suspend fun refreshThere() = engine().refreshControlledOutputs()

    companion object {
        /** How long a search waits for answers: the multicast lock's window. */
        const val SEARCH_MS = 5_000L
    }
}

/** What the renderers' list says when it has none and discovery is allowed. */
internal fun noRenderers(searching: Boolean) = when {
    searching -> "Looking for speakers and streamers on this network…"
    else -> "No UPnP speakers or streamers found on this network. Turn one on, and it appears here."
}

/**
 * The Output half of the sheet, for the device in view: the outputs the engine
 * publishes as this app's own, which while another device is controlled are
 * that device's (`owner`), as the Mac and iOS pickers read them. A phone's own
 * output is the route the system chose, so it is one row; a computer lists its
 * system default and each output it has, with the EQ preset each is set to.
 * Renderers follow.
 */
@Composable
fun OutputSection(picker: OutputPicker, outputs: OutputsInfo?, renderer: RendererOutput?, blocked: Boolean, controlled: String?, platform: String?) {
    when {
        outputs?.owner != null -> ControlledOutputs(picker, outputs, platform)
        // Controlling a device that has not said what it has: reconnecting,
        // or out of reach. This phone's own outputs are not the question.
        controlled != null -> Column {
            KoanText("Output on $controlled", inset.padding(top = KoanSpace.l), role = KoanType.TitleSmall, tone = KoanTone.Strong, appText = true)
            KoanText(
                "$controlled's outputs appear here once it is reachable.",
                inset.padding(vertical = KoanSpace.s),
                role = KoanType.Fine,
                tone = KoanTone.Muted,
            )
        }
        else -> OwnOutputs(picker, outputs, renderer, blocked)
    }
}

/** The controlled device's outputs, each picked there by a command. */
@Composable
private fun ControlledOutputs(picker: OutputPicker, outputs: OutputsInfo, platform: String?) {
    val state by picker.state.collectAsStateWithLifecycle()
    Column {
        KoanText("Output on ${outputs.owner}", inset.padding(top = KoanSpace.l), role = KoanType.TitleSmall, tone = KoanTone.Strong, appText = true)
        state.refused?.let { KoanText(it, inset.padding(vertical = KoanSpace.s), role = KoanType.Fine, tone = KoanTone.Bad) }
        // A phone's or a television's output is its route, which its system
        // chooses: listed, not picked between.
        val routed = platform == "ios" || platform == "tvos" || platform == "android"
        if (routed) {
            outputs.devices.forEach { d ->
                OutputRow(d.name, presetSummary(d, outputs), selected = outputs.current == OutputChoice.Default, onSelect = { picker.chooseThere(OutputChoice.Default) })
            }
        } else {
            OutputRow("System Default", null, selected = outputs.current == OutputChoice.Default, onSelect = { picker.chooseThere(OutputChoice.Default) })
            outputs.devices.forEach { d ->
                val choice = OutputChoice.Device(d.id)
                OutputRow(d.name, presetSummary(d, outputs), selected = outputs.current == choice, onSelect = { picker.chooseThere(choice) })
            }
        }
        outputs.renderers.forEach { r ->
            val choice = OutputChoice.Renderer(r.id)
            val selected = outputs.current == choice
            OutputRow(
                name = r.name,
                detail = if (!selected && r.busy) BUSY else listOf(upnpKind(r), presetSummary(r, outputs)).filterNotNull().joinToString(" · "),
                selected = selected,
                warning = !selected && r.busy,
                onSelect = { picker.chooseThere(choice) },
            )
        }
    }
}

/** This phone's own output, then the renderers the engine has found, each saying how it is going. */
@Composable
private fun OwnOutputs(picker: OutputPicker, outputs: OutputsInfo?, renderer: RendererOutput?, blocked: Boolean) {
    val scope = rememberCoroutineScope()
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
        when {
            // Discovery is refused: neither devices nor renderers can be found.
            blocked -> LocalNetworkBlocked()
            renderers.isEmpty() -> KoanText(noRenderers(state.searching), inset.padding(vertical = KoanSpace.s), role = KoanType.Fine, tone = KoanTone.Muted)
        }
        if (!blocked && !state.searching) {
            ButtonRow { KoanButton("Search again", { scope.launch { picker.search() } }, kind = KoanButtonKind.Link) }
        }
    }
}

private const val BUSY = "In use by something else. Picking it takes over."

private fun upnpKind(r: OutputInfo) = listOf(r.detail.ifEmpty { null }, "UPnP").filterNotNull().joinToString(" · ")

/**
 * The EQ preset an output is set to, in a word or two, as the device
 * published it: its name, edited, Flat, or Unsaved. Nothing where the device
 * has no presets to choose from.
 */
internal fun presetSummary(o: OutputInfo, outputs: OutputsInfo): String? {
    if (outputs.profiles.isEmpty() && o.preset == null && !o.unsaved) return null
    o.preset?.let { return if (o.unsaved) "$it, edited" else it }
    return if (o.unsaved) "Unsaved" else "Flat"
}

/** A renderer's line: how picking it is going, else what it is. */
internal fun rendererDetail(r: OutputInfo, output: OutputState): String {
    val what = listOf(r.detail.ifEmpty { null }, "UPnP").filterNotNull().joinToString(" · ")
    return when {
        output is OutputState.Connecting && output.udn == r.id -> "Connecting…"
        output is OutputState.Failed && output.udn == r.id -> "Couldn't play here: ${output.reason}"
        output is OutputState.Playing && output.udn == r.id -> output.problem ?: "Playing here · $what"
        r.busy -> BUSY
        else -> what
    }
}

/** The local network permission refused, with the way to the setting, as the Devices page has it. */
@Composable
internal fun LocalNetworkBlocked() {
    val context = LocalContext.current
    Row(inset.fillMaxWidth().padding(vertical = KoanSpace.s), horizontalArrangement = Arrangement.spacedBy(KoanSpace.s), verticalAlignment = Alignment.CenterVertically) {
        KoanText(LOCAL_NETWORK_BLOCKED, Modifier.weight(1f), role = KoanType.Fine, tone = KoanTone.Bad)
        KoanButton("Settings", {
            context.startActivity(Intent(SystemSettings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", context.packageName, null)))
        }, kind = KoanButtonKind.Compact)
    }
}

internal const val LOCAL_NETWORK_BLOCKED = "Allow nearby devices in Settings to find kōan devices on this network."

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
