package cc.blit.koan.ui.pages

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.theme.KoanButton
import cc.blit.koan.theme.KoanButtonKind
import cc.blit.koan.theme.KoanIcon
import cc.blit.koan.theme.KoanIconButton
import cc.blit.koan.theme.KoanRole
import cc.blit.koan.theme.KoanRule
import cc.blit.koan.theme.KoanShape
import cc.blit.koan.theme.KoanSpace
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanTheme
import cc.blit.koan.theme.KoanTone
import cc.blit.koan.ui.playlists.ConfirmDeleteDialog
import cc.blit.koan.ui.playlists.NameDialog
import cc.blit.koan.ui.playlists.NewPlaylistHost
import cc.blit.koan.ui.playlists.PlaylistLayout
import cc.blit.koan.ui.playlists.PlaylistMosaic
import cc.blit.koan.ui.playlists.PlaylistReport
import kotlinx.coroutines.delay
import sh.calvin.reorderable.ReorderableItem
import sh.calvin.reorderable.rememberReorderableLazyListState
import uniffi.koan_ffi.Playlist

/**
 * The list of playlists. The Mac has no page for it (its sidebar is the
 * list); this is `PlaylistsList.swift`, with the reordering the sidebar does.
 *
 * Tap opens the playlist (its rows are read before the page moves). The handle
 * at a row's end reorders; the order shown is held until the model's own new
 * order arrives so the row does not spring back. Rename and delete sit behind
 * the row's more button; a playlist read from a file keeps the name its file
 * gives it, so rename is greyed there. Making one asks for a name through
 * `PlaylistsModel.naming`, answered by [NewPlaylistHost].
 *
 * Not built yet: dragging a track onto a row (no source for it on a phone;
 * "add to playlist" menus on the other pages set `naming` and call `add`).
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun PlaylistsPage(runtime: KoanRuntime) {
    val model = runtime.playlists
    val playlists by model.playlists.collectAsStateWithLifecycle()
    val covers by model.covers.collectAsStateWithLifecycle()

    // Free when nothing moved under it; the model keeps a mosaic until the playlist changes.
    LaunchedEffect(Unit) { model.load() }

    var expanded by remember { mutableStateOf<Long?>(null) }
    var renaming by remember { mutableStateOf<Playlist?>(null) }
    var deleting by remember { mutableStateOf<Playlist?>(null) }

    // What is shown while a row is dragged, and until the model's own order arrives.
    var dragging by remember { mutableStateOf<List<Playlist>?>(null) }
    LaunchedEffect(playlists) { dragging = null }
    val shown = dragging ?: playlists

    val listState = rememberLazyListState()
    val reorder = rememberReorderableLazyListState(listState) { from, to ->
        val current = dragging ?: playlists
        val a = current.indexOfFirst { it.id == from.key }
        val b = current.indexOfFirst { it.id == to.key }
        if (a >= 0 && b >= 0 && a != b) dragging = current.toMutableList().apply { add(b, removeAt(a)) }
    }

    Column(Modifier.fillMaxSize()) {
        Row(
            Modifier.fillMaxWidth().padding(start = KoanSpace.page, end = KoanSpace.xs),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(Modifier.weight(1f).padding(vertical = KoanSpace.m)) {
                KoanText("playlists", KoanRole.Body, KoanTone.Strong)
                KoanText(PlaylistLayout.count(playlists.size, "playlist"), KoanRole.Fine, KoanTone.Muted, data = true)
            }
            KoanIconButton("plus", "New playlist", { model.beginNaming() })
        }
        KoanRule()
        if (playlists.isEmpty()) {
            Column(
                Modifier.fillMaxSize().padding(KoanSpace.page),
                verticalArrangement = Arrangement.Center,
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                KoanIcon("playlist", tone = KoanTone.Muted, textSizeDp = 22f)
                KoanText("No playlists", KoanRole.Body, KoanTone.Strong, modifier = Modifier.padding(top = KoanSpace.m))
                KoanText(
                    "Made here or on your server, they show up in both.",
                    KoanRole.Meta, KoanTone.Muted, modifier = Modifier.padding(top = KoanSpace.s), maxLines = 3,
                )
            }
        } else {
            LazyColumn(state = listState, modifier = Modifier.fillMaxSize()) {
                itemsIndexed(shown, key = { _, p -> p.id }, contentType = { _, _ -> "playlist" }) { _, playlist ->
                    ReorderableItem(reorder, key = playlist.id) {
                        val open = expanded == playlist.id
                        Column(Modifier.fillMaxWidth()) {
                            Row(
                                Modifier
                                    .fillMaxWidth()
                                    .clickable(role = Role.Button) { runtime.navigator.openPlaylist(playlist.id) }
                                    .padding(start = KoanSpace.page, end = KoanSpace.xs, top = KoanSpace.s, bottom = KoanSpace.s),
                                verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.spacedBy(KoanSpace.m),
                            ) {
                                PlaylistMosaic(covers[playlist.id].orEmpty(), 48.dp)
                                Column(Modifier.weight(1f)) {
                                    KoanText(playlist.name, KoanRole.Body, KoanTone.Strong, data = true)
                                    KoanText(rowDetail(playlist), KoanRole.Fine, KoanTone.Muted, data = true)
                                }
                                KoanIconButton(
                                    "more", "More about ${playlist.name}", { expanded = if (open) null else playlist.id },
                                    tone = if (open) KoanTone.Accent else KoanTone.Ink,
                                )
                                Box(
                                    Modifier.size(KoanShape.hit).draggableHandle(
                                        onDragStopped = {
                                            val order = playlists.map { it.id }
                                            val now = dragging ?: playlists
                                            val move = PlaylistLayout.movePlaylist(
                                                order, from = order.indexOf(playlist.id), to = now.indexOfFirst { it.id == playlist.id },
                                            )
                                            if (move == null) dragging = null else model.reorderPlaylists(move.moving, move.onto)
                                        },
                                    ),
                                    contentAlignment = Alignment.Center,
                                ) { KoanIcon("chevron-up-down", tone = KoanTone.Muted, description = "Reorder") }
                            }
                            if (open) {
                                Row(
                                    Modifier.fillMaxWidth().background(KoanTheme.colors.surface).padding(horizontal = KoanSpace.page),
                                    horizontalArrangement = Arrangement.spacedBy(KoanSpace.s),
                                ) {
                                    // Its file names it.
                                    KoanButton(
                                        "rename", { renaming = playlist; expanded = null },
                                        kind = KoanButtonKind.Compact, enabled = PlaylistLayout.renamable(playlist),
                                    )
                                    KoanButton(
                                        "delete", { deleting = playlist; expanded = null },
                                        kind = KoanButtonKind.Destructive, icon = "remove",
                                    )
                                }
                            }
                            KoanRule()
                        }
                    }
                }
            }
        }
    }

    renaming?.let { target ->
        NameDialog(
            title = "rename playlist", message = null, initial = target.name, confirm = "rename",
            onDismiss = { renaming = null },
            onConfirm = { name -> model.rename(target.id, name); renaming = null },
        )
    }
    deleting?.let { target ->
        ConfirmDeleteDialog(
            PlaylistLayout.deleteQuestion(target),
            onDismiss = { deleting = null },
            onConfirm = {
                model.delete(target.id)
                runtime.navigator.forget(cc.blit.koan.nav.Section.Playlist(target.id))
                deleting = null
            },
        )
    }
    NewPlaylistHost(runtime)
    PlaylistReport(runtime)
}

/** "12 tracks · 48:05", with what makes a playlist different from an ordinary one named. */
private fun rowDetail(playlist: Playlist): String = buildList {
    add(PlaylistLayout.count(playlist.trackCount.toInt()))
    if (playlist.durationMs > 0) add(PlaylistLayout.duration(playlist.durationMs))
    when {
        playlist.smart -> add("smart")
        playlist.fromFile -> add("file")
        playlist.readonly -> add("read-only")
    }
}.joinToString(" · ")
