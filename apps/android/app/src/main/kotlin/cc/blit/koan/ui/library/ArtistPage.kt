package cc.blit.koan.ui.library

import androidx.compose.animation.animateContentSize
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import cc.blit.koan.art.Art
import cc.blit.koan.ffi.Album
import cc.blit.koan.ffi.Artist
import cc.blit.koan.ffi.ArtistInfo
import cc.blit.koan.ffi.Placement
import cc.blit.koan.ffi.ReleaseSection
import cc.blit.koan.ffi.TrackSet
import cc.blit.koan.ffi.releaseSectionTitle
import cc.blit.koan.ui.StaleNotice
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.shown
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import cc.blit.koan.ui.theme.KoanUnavailable
import cc.blit.koan.ui.theme.LocalKoanTokens
import cc.blit.koan.ui.value

/**
 * An artist: their photograph and name, play and shuffle, the opening of their
 * biography, and their records as a grid, since that is how a discography is
 * thought of. An artist credited only on other people's records gets those
 * tracks instead of an empty grid. The select button picks several of either.
 */
@Composable
fun ArtistPage(artistId: Long) {
    val app = koanApp()
    val nav = LocalNavigator.current
    val model = viewModel { ArtistModel(app.mirror, artistId) }
    val read by model.record.state.collectAsStateWithLifecycle()
    val info by model.info.collectAsStateWithLifecycle()
    val looking by model.looking.collectAsStateWithLifecycle()
    val selection = model.selection
    SelectMode(selection)
    val gutter = LocalKoanTokens.current.gutter
    val grid = rememberLazyGridState()
    val list = rememberLazyListState()
    val (current, playing) = nowPlayingTrack()
    val held = read.value
    // The name moves into the bar once the header has scrolled away.
    val scrolled by remember { derivedStateOf { grid.firstVisibleItemIndex > 0 || list.firstVisibleItemIndex > 0 } }
    Column {
        PageBar(if (scrolled) held?.artist?.name.orEmpty() else "", appText = false) {
            if (held != null && (held.albums.isNotEmpty() || held.features.isNotEmpty() || held.appearances.isNotEmpty())) SelectButton(selection)
        }
        read.StaleNotice("this artist", model.record::retry)
        Box(Modifier.weight(1f)) {
            if (held == null) {
                read.shown("this artist", model.record::retry)
                return@Box
            }
            val artist = held.artist ?: return@Box KoanUnavailable("This artist isn't in the library any more", KoanIcons.artist, null)
            if (held.albums.isEmpty() && held.features.isEmpty() && held.appearances.isNotEmpty()) {
                val ids = remember(held) { held.appearances.map { it.id } }
                val records = held.appearances.mapNotNull { it.albumId }.toSet().size
                LazyColumn(state = list, modifier = Modifier.fillMaxSize()) {
                    item(key = "header") {
                        Header(artistId, artist, "Appears on ${count(records.toLong(), "album")}", info, looking, Modifier.padding(horizontal = gutter))
                    }
                    itemsIndexed(held.appearances, key = { _, t -> t.id }) { index, track ->
                        TrackRow(
                            track,
                            lead = "${index + 1}",
                            current = track.id == current,
                            playing = playing,
                            onPlay = { app.playFrom(ids, index, nav) },
                            selection = selection,
                            mixed = true,
                            onArtistPage = artistId,
                        )
                    }
                }
            } else {
                LazyVerticalGrid(
                    columns = GridCells.Adaptive(150.dp),
                    state = grid,
                    contentPadding = PaddingValues(start = gutter - 4.dp, end = gutter - 4.dp, bottom = KoanSpace.l),
                    modifier = Modifier.fillMaxSize(),
                ) {
                    item(key = "header", span = { GridItemSpan(maxLineSpan) }) {
                        Header(artistId, artist, count(held.albums.size.toLong(), "album"), info, looking, Modifier.padding(horizontal = 4.dp))
                    }
                    // A discography of albums alone, or one nobody tagged, needs no headings.
                    // The core lists them section by section, so a section is a run of them.
                    val sections = held.albums.fold(mutableListOf<Pair<ReleaseSection, MutableList<Album>>>()) { runs, album ->
                        runs.also {
                            if (it.lastOrNull()?.first == album.releaseSection) it.last().second += album
                            else it += album.releaseSection to mutableListOf(album)
                        }
                    }
                    sections.forEachIndexed { i, (section, albums) ->
                        if (sections.size > 1) {
                            item(key = "section-$section", span = { GridItemSpan(maxLineSpan) }) {
                                KoanText(
                                    releaseSectionTitle(section),
                                    Modifier.padding(start = 4.dp, end = 4.dp, top = if (i == 0) 0.dp else KoanSpace.l),
                                    role = KoanType.Control,
                                    appText = true,
                                )
                            }
                        }
                        items(albums, key = { it.id }, contentType = { "album" }) { AlbumTile(it, selection = selection, showArtist = false) }
                    }
                    if (held.features.isNotEmpty()) {
                        item(key = "features", span = { GridItemSpan(maxLineSpan) }) {
                            FeaturesOn(held.features, Modifier.padding(start = 4.dp, end = 4.dp, top = KoanSpace.l))
                        }
                        items(held.features, key = { "feature-${it.id}" }, contentType = { "album" }) { AlbumTile(it, selection = selection, onArtistPage = artistId) }
                    }
                }
            }
        }
        if (selection.active) SelectionBar(selection)
    }
}

@Composable
private fun Header(artistId: Long, artist: Artist, counts: String, info: ArtistInfo?, looking: Boolean, modifier: Modifier) {
    val app = koanApp()
    var menu by remember { mutableStateOf(false) }
    val playable = Playable.OfArtist(artist.id, artist.name)
    PageHeader(
        artist.name,
        modifier.animateContentSize(),
        art = if (info?.hasImage == true) ({ Sleeve(Art.artist(artistId), size = 160.dp) }) else null,
        detail = { KoanText(counts, role = KoanType.Fine, tone = KoanTone.Muted) },
        actions = {
            KoanButton("Play", { app.play(playable) }, kind = KoanButtonKind.Prominent, icon = KoanIcons.play)
            KoanButton("Shuffle", { app.play(playable, shuffled = true) }, icon = KoanIcons.shuffle)
            ArtistHeart(artist.id, labelled = true)
            Box {
                KoanButton("More", { menu = true }, kind = KoanButtonKind.Icon, icon = KoanIcons.more)
                PlayableMenu(playable, menu) { menu = false }
            }
        },
        below = {
            val bio = info?.bio
            when {
                bio != null -> Bio(bio, info.bioUrl, info.imageCredit)
                looking -> KoanText("Looking up biography…", Modifier.padding(bottom = KoanSpace.m), role = KoanType.Meta, tone = KoanTone.Muted, appText = true)
            }
        },
    )
}

/**
 * The heading over the records an artist features on, with their own play and
 * queue: compact beside the page's one prominent Play, which is for the
 * artist's own records.
 */
@Composable
private fun FeaturesOn(albums: List<Album>, modifier: Modifier) {
    val app = koanApp()
    val ids = albums.map { it.id }
    Row(modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(KoanSpace.s), verticalAlignment = Alignment.CenterVertically) {
        KoanText("Features on", Modifier.weight(1f), role = KoanType.Control, appText = true)
        KoanButton("Play", { app.player.playNow("Features on", ids.map { TrackSet.Album(it) }) }, kind = KoanButtonKind.Compact, icon = KoanIcons.play)
        KoanButton("Queue", { app.player.queue(ids.map { TrackSet.Album(it) }, Placement.Append) }, kind = KoanButtonKind.Compact, icon = KoanIcons.queue)
    }
}

/** The opening of the artist's Wikipedia article: a few lines, a tap for the rest, credited as its licence asks. */
@Composable
private fun Bio(bio: String, source: String?, imageCredit: String?) {
    var expanded by rememberSaveable { mutableStateOf(false) }
    val uri = LocalUriHandler.current
    Column(
        Modifier
            .padding(bottom = KoanSpace.m)
            .clickable(remember { MutableInteractionSource() }, indication = null) { expanded = !expanded },
        verticalArrangement = Arrangement.spacedBy(KoanSpace.xs),
    ) {
        // The extract separates paragraphs with a single newline.
        KoanText(bio.replace("\n", "\n\n"), role = KoanType.Meta, tone = KoanTone.Muted, maxLines = if (expanded) Int.MAX_VALUE else 4)
        if (expanded) {
            Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.m), verticalAlignment = Alignment.CenterVertically) {
                if (source != null) KoanButton("From Wikipedia", { uri.openUri(source) }, kind = KoanButtonKind.Link)
                if (imageCredit != null) KoanText("Photo: $imageCredit", role = KoanType.Fine, tone = KoanTone.Muted)
            }
        } else {
            KoanButton("More", { expanded = true }, kind = KoanButtonKind.Link)
        }
    }
}
