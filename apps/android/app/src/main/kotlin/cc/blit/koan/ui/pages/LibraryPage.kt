package cc.blit.koan.ui.pages

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.sizeIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.nav.Route
import cc.blit.koan.nav.Section
import cc.blit.koan.theme.KoanButton
import cc.blit.koan.theme.KoanButtonKind
import cc.blit.koan.theme.KoanIcon
import cc.blit.koan.theme.KoanRole
import cc.blit.koan.theme.KoanRule
import cc.blit.koan.theme.KoanShape
import cc.blit.koan.theme.KoanSpace
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanTone
import uniffi.koan_ffi.Playlist
import uniffi.koan_ffi.Stats

/**
 * The Library tab's root: the sections the library browses by (the Mac's
 * sidebar), then the playlists. A row opens its page on this tab. The counts
 * are `library.stats`, read once when the page is shown.
 */
@Composable
fun LibraryPage(runtime: KoanRuntime) {
    val library = runtime.library
    val navigator = runtime.navigator
    val stats by library.stats.collectAsStateWithLifecycle()
    val playlists by runtime.playlists.playlists.collectAsStateWithLifecycle()
    LaunchedEffect(Unit) { library.loadStats() }

    LazyColumn(Modifier.fillMaxSize()) {
        item(key = "title", contentType = "title") {
            KoanText("Library", KoanRole.Title, KoanTone.Strong, modifier = Modifier.padding(KoanSpace.page))
        }
        items(Section.library, key = { "section:${it::class.simpleName}" }, contentType = { "section" }) { section ->
            NavRow(section.iconName, section.title, sectionDetail(section, stats)) { navigator.show(section) }
        }
        item(key = "playlists", contentType = "heading") {
            Row(
                Modifier.fillMaxWidth().padding(start = KoanSpace.page, end = KoanSpace.s, top = KoanSpace.xl),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                KoanText("Playlists", KoanRole.TitleSm, KoanTone.Strong, modifier = Modifier.weight(1f))
                KoanButton("all", { navigator.go(Route.Playlists) }, kind = KoanButtonKind.Text)
            }
        }
        if (playlists.isEmpty()) {
            item(key = "no-playlists", contentType = "note") {
                KoanText(
                    "No playlists yet.", KoanRole.Meta, KoanTone.Muted,
                    modifier = Modifier.padding(horizontal = KoanSpace.page, vertical = KoanSpace.s),
                )
            }
        } else {
            items(playlists, key = { "playlist:${it.id}" }, contentType = { "playlist" }) { playlist ->
                NavRow("playlist", playlist.name, playlistDetail(playlist), data = true) { navigator.openPlaylist(playlist.id) }
            }
        }
    }
}

/** The count a section row shows, where the library's stats give one. */
internal fun sectionDetail(section: Section, stats: Stats?): String? = when (section) {
    Section.Albums -> stats?.let { countLabel(it.totalAlbums, "album") }
    Section.Artists -> stats?.let { countLabel(it.totalArtists, "artist") }
    Section.Tracks -> stats?.let { countLabel(it.totalTracks, "track") }
    else -> null
}

internal fun playlistDetail(playlist: Playlist): String = countLabel(playlist.trackCount.toLong(), "track")

@Composable
private fun NavRow(icon: String, title: String, detail: String?, data: Boolean = false, onClick: () -> Unit) {
    Column(Modifier.fillMaxWidth()) {
        Row(
            Modifier.fillMaxWidth().sizeIn(minHeight = KoanShape.hit + KoanSpace.s)
                .clickable(role = Role.Button, onClick = onClick)
                .padding(horizontal = KoanSpace.page, vertical = KoanSpace.s),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(KoanSpace.m),
        ) {
            KoanIcon(icon, tone = KoanTone.Muted, textSizeDp = 18f)
            KoanText(title, KoanRole.Body, KoanTone.Strong, data = data, modifier = Modifier.weight(1f))
            if (detail != null) KoanText(detail, KoanRole.Fine, KoanTone.Muted)
            KoanIcon("chevron-right", tone = KoanTone.Muted, textSizeDp = 14f)
        }
        KoanRule(Modifier.padding(horizontal = KoanSpace.page))
    }
}
