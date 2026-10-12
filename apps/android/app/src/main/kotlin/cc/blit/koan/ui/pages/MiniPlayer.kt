package cc.blit.koan.ui.pages

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.theme.KoanRole
import cc.blit.koan.theme.KoanRule
import cc.blit.koan.theme.KoanSpace
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanTone

/**
 * The mini player the shell puts directly above the tab bar, with its own top
 * rule: the playhead as a 2 dp accent line, the sleeve, title and transport,
 * opening the now-playing sheet. Draws nothing with nothing queued.
 *
 * Placeholder: the title and artist only.
 */
@Composable
fun MiniPlayer(runtime: KoanRuntime) {
    val playback by runtime.mirror.playback.collectAsStateWithLifecycle()
    val entry = playback.entry ?: return
    Column(Modifier.fillMaxWidth()) {
        KoanRule()
        Column(Modifier.padding(horizontal = KoanSpace.page, vertical = KoanSpace.s)) {
            KoanText(entry.title, KoanRole.Meta, KoanTone.Strong, data = true)
            KoanText(entry.artist, KoanRole.Fine, KoanTone.Muted, data = true)
        }
    }
}
