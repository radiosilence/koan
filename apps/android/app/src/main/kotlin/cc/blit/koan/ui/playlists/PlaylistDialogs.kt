package cc.blit.koan.ui.playlists

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.window.Dialog
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.theme.KoanButton
import cc.blit.koan.theme.KoanButtonKind
import cc.blit.koan.theme.KoanRole
import cc.blit.koan.theme.KoanShape
import cc.blit.koan.theme.KoanSpace
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanTheme
import cc.blit.koan.theme.KoanTone
import kotlinx.coroutines.launch

/**
 * The one place a new playlist is named: the port of `NewPlaylistAlert`.
 * Everything that wants a playlist made sets `PlaylistsModel.naming`; this
 * presents it. The request is a model value rather than a dialog because the
 * gesture that asked (a menu) is gone before the answer is needed.
 *
 * Hosted by the playlist pages for now, which is enough for the requests the
 * pages make. A request from a page that does not host it (the queue's "add to
 * playlist") needs this called once from the app shell, and then the pages
 * must stop calling it, or two dialogs would answer one request. Say so in the
 * shell's change, not here.
 */
@Composable
fun NewPlaylistHost(runtime: KoanRuntime) {
    val naming by runtime.playlists.naming.collectAsStateWithLifecycle()
    val trackIds = naming ?: return
    val scope = rememberCoroutineScope()
    NameDialog(
        title = "new playlist",
        message = when (trackIds.size) {
            0 -> "Give it a name. Add records and tracks from their menus afterwards."
            1 -> "One track goes in. Give the playlist a name."
            else -> "${PlaylistLayout.count(trackIds.size)} go in. Give the playlist a name."
        },
        initial = "",
        confirm = "create",
        onDismiss = { runtime.playlists.endNaming() },
        onConfirm = { name ->
            runtime.playlists.endNaming()
            scope.launch {
                // You made it to put something in it, so go and look at it.
                runtime.playlists.create(name, trackIds)?.let { runtime.navigator.openPlaylist(it.id) }
            }
        },
    )
}

/** A report the model wants shown (a failed create, a refused edit), until it is dismissed. */
@Composable
fun PlaylistReport(runtime: KoanRuntime) {
    val report by runtime.playlists.report.collectAsStateWithLifecycle()
    val text = report ?: return
    PlaylistDialog(
        title = "playlist",
        message = text,
        onDismiss = { runtime.playlists.dismissReport() },
        actions = { KoanButton("ok", { runtime.playlists.dismissReport() }, kind = KoanButtonKind.Text) },
    )
}

/** A name asked for: new playlist or rename. Confirming an empty or blank name does nothing. */
@Composable
fun NameDialog(
    title: String,
    message: String?,
    initial: String,
    confirm: String,
    onDismiss: () -> Unit,
    onConfirm: (String) -> Unit,
) {
    var name by remember { mutableStateOf(initial) }
    val focus = remember { FocusRequester() }
    val valid = PlaylistLayout.nameable(name)
    val submit: () -> Unit = { valid?.let(onConfirm) }
    LaunchedEffect(Unit) { focus.requestFocus() }
    PlaylistDialog(title, message, onDismiss, actions = {
        KoanButton("cancel", onDismiss, kind = KoanButtonKind.Text)
        KoanButton(confirm, submit, kind = KoanButtonKind.Standard, enabled = valid != null)
    }) {
        BasicTextField(
            value = name,
            onValueChange = { name = it },
            singleLine = true,
            textStyle = KoanTheme.style(KoanRole.Body).copy(color = KoanTheme.colors.strong),
            cursorBrush = SolidColor(KoanTheme.colors.accent),
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
            keyboardActions = KeyboardActions(onDone = { submit() }),
            modifier = Modifier
                .fillMaxWidth()
                .focusRequester(focus)
                .border(KoanShape.hairline, KoanTheme.colors.muted, RectangleShape)
                .padding(horizontal = KoanSpace.m, vertical = KoanSpace.m),
        )
    }
}

/** Are-you-sure for a delete; the playlist goes, its tracks do not. */
@Composable
fun ConfirmDeleteDialog(question: String, onDismiss: () -> Unit, onConfirm: () -> Unit) {
    PlaylistDialog("delete playlist", question, onDismiss, actions = {
        KoanButton("cancel", onDismiss, kind = KoanButtonKind.Text)
        KoanButton("delete", onConfirm, kind = KoanButtonKind.Destructive)
    })
}

@Composable
private fun PlaylistDialog(
    title: String,
    message: String?,
    onDismiss: () -> Unit,
    actions: @Composable () -> Unit,
    body: @Composable () -> Unit = {},
) {
    Dialog(onDismissRequest = onDismiss) {
        Column(
            Modifier
                .fillMaxWidth()
                .background(KoanTheme.colors.surface, RectangleShape)
                .border(KoanShape.hairline, KoanTheme.colors.rule, RectangleShape)
                .padding(KoanSpace.l),
            verticalArrangement = Arrangement.spacedBy(KoanSpace.m),
        ) {
            KoanText(title, KoanRole.Body, KoanTone.Strong)
            if (message != null) KoanText(message, KoanRole.Meta, KoanTone.Muted, maxLines = 5, data = true)
            body()
            Row(
                Modifier.fillMaxWidth().padding(top = KoanSpace.xs),
                horizontalArrangement = Arrangement.spacedBy(KoanSpace.s, Alignment.End),
                verticalAlignment = Alignment.CenterVertically,
            ) { actions() }
        }
    }
}
