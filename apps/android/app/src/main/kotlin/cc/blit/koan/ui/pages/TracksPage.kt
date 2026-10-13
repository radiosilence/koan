package cc.blit.koan.ui.pages

import androidx.compose.runtime.Composable
import cc.blit.koan.engine.KoanRuntime

/** The track browser (Section.Tracks). */
@Composable
fun TracksPage(runtime: KoanRuntime) {
    PlaceholderPage("tracks", "runtime.library.visibleTracks, trackTotal, trackSort; play through runtime.player.")
}
