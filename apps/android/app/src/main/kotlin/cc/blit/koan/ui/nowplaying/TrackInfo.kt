package cc.blit.koan.ui.nowplaying

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.BottomSheetDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Modifier
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.unit.dp
import cc.blit.koan.ffi.InfoField
import cc.blit.koan.ffi.TrackDetails
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.settings.inset
import cc.blit.koan.ui.theme.KoanSectionHeader
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import kotlinx.coroutines.CancellationException

/** Opens a track's info sheet from any menu. Provided by `TrackInfoHost`. */
val LocalShowTrackInfo = staticCompositionLocalOf<(Long) -> Unit> { error("no TrackInfoHost: compose inside AppShell") }

/**
 * The one place a track's info sheet is opened from a menu: held above the
 * menus, since a menu composed only while it is open would take its sheet
 * with it as it closes.
 */
@Composable
fun TrackInfoHost(content: @Composable () -> Unit) {
    val mirror = koanApp().mirror
    var shown by remember { mutableStateOf<Long?>(null) }
    val show = remember { { trackId: Long -> shown = trackId } }
    CompositionLocalProvider(LocalShowTrackInfo provides show, content = content)
    shown?.let { trackId ->
        // Read again when the library changes, so a rescan or sync shows in the open sheet.
        val library by mirror.library.collectAsStateWithLifecycle()
        val details by produceState<TrackDetails?>(null, trackId, library) {
            try {
                value = mirror.engine().trackDetails(trackId)
                // Gone from the library: there is nothing to show.
                if (value == null) shown = null
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                mirror.report(e)
                shown = null
            }
        }
        details?.let { TrackInfoSheet(it) { shown = null } }
    }
}

/**
 * Everything the library knows about a track, as each of its sources says it:
 * the file's tags, the server's entry, the ReplayGain in the file, and its ids.
 * Small and dense, for reading a value off or copying it, as iOS's
 * `TrackInfoView` has it.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun TrackInfoSheet(info: TrackDetails, onDismiss: () -> Unit) {
    val on = KoanTheme.isOn
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        shape = if (on) RectangleShape else BottomSheetDefaults.ExpandedShape,
        containerColor = if (on) KoanTheme.colors.bg else BottomSheetDefaults.ContainerColor,
        tonalElevation = 0.dp,
    ) {
        SelectionContainer {
            Column(
                Modifier.verticalScroll(rememberScrollState()).navigationBarsPadding().padding(bottom = KoanSpace.l),
                verticalArrangement = Arrangement.spacedBy(KoanSpace.l),
            ) {
                KoanText(info.track.title, inset, tone = KoanTone.Strong)
                info.sources.forEach { source -> Group(if (source.kind == "server") "Server" else "File", source.fields) }
                if (info.replayGain.isNotEmpty()) Group("ReplayGain", info.replayGain)
                Group("Library", listOfNotNull(InfoField("Track id", info.track.id.toString()), info.uid?.let { InfoField("Uid", it) }))
            }
        }
    }
}

@Composable
private fun Group(title: String, fields: List<InfoField>) {
    Column(verticalArrangement = Arrangement.spacedBy(3.dp)) {
        KoanSectionHeader(title, inset)
        fields.forEach { field ->
            Row(inset, horizontalArrangement = Arrangement.spacedBy(KoanSpace.s)) {
                KoanText(field.name, Modifier.width(128.dp), role = KoanType.Fine, tone = KoanTone.Muted)
                KoanText(field.value, Modifier.weight(1f), role = KoanType.Fine)
            }
        }
    }
}
