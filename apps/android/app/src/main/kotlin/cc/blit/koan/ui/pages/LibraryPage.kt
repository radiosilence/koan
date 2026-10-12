package cc.blit.koan.ui.pages

import androidx.compose.runtime.Composable
import cc.blit.koan.engine.KoanRuntime

/** The Library tab's own page: sections and playlists. */
@Composable
fun LibraryPage(runtime: KoanRuntime) {
    PlaceholderPage("library", "Lists Section.library and the playlists (runtime.playlists.playlists), each opening runtime.navigator.show(section) / openPlaylist(id).")
}
