package cc.blit.koan.ui.downloads

import androidx.compose.runtime.Composable
import cc.blit.koan.engine.EngineMirror
import cc.blit.koan.ffi.KoanEngine
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.playlists.MenuItem
import cc.blit.koan.ui.theme.KoanIcons

/**
 * Fetching for offline, and throwing the copies away. Both skip what does not
 * apply: fetching passes over what is already here, removing over what is not.
 */
object Downloads {
    fun fetch(mirror: EngineMirror, trackIds: suspend KoanEngine.() -> List<Long>) =
        mirror.launch { trackIds().takeIf { it.isNotEmpty() }?.let { downloadToCache(it) } }

    fun remove(mirror: EngineMirror, trackIds: suspend KoanEngine.() -> List<Long>) =
        mirror.launch { trackIds().takeIf { it.isNotEmpty() }?.let { clearDownloads(it) } }
}

/**
 * A menu's download items, offered according to what is here. One track knows
 * whether its bytes are on the device (`onDisk`), so it gets one verb; a record,
 * a playlist or a selection (`onDisk = null`) is usually partly there, and gets
 * both.
 *
 * `trackIds` is resolved against the engine when an item is picked; `dismiss`
 * closes the menu.
 */
@Composable
fun DownloadMenuItems(onDisk: Boolean?, dismiss: () -> Unit, trackIds: suspend KoanEngine.() -> List<Long>) {
    val mirror = koanApp().mirror
    if (onDisk != true) {
        MenuItem(if (onDisk == null) "Download All" else "Download", KoanIcons.downloads) {
            dismiss()
            Downloads.fetch(mirror, trackIds)
        }
    }
    if (onDisk != false) {
        MenuItem(if (onDisk == null) "Remove Downloads" else "Remove Download", KoanIcons.clear) {
            dismiss()
            Downloads.remove(mirror, trackIds)
        }
    }
}
