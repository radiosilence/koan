package cc.blit.koan.ui.playlists

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import cc.blit.koan.R
import cc.blit.koan.art.Art
import cc.blit.koan.ui.navigation.LocalNavigator
import coil3.compose.AsyncImage

/** The bar over a page the library opens: back, the page's name, its actions. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PageBar(title: String, actions: @Composable RowScope.() -> Unit = {}) {
    val nav = LocalNavigator.current
    TopAppBar(
        title = { Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis) },
        navigationIcon = {
            IconButton(onClick = { nav.pop() }) { Icon(painterResource(R.drawable.koan_chevron_left), "Back") }
        },
        actions = actions,
    )
}

/**
 * A playlist's face: the covers of up to four of its records, two by two, or
 * the one cover when it holds fewer than four.
 */
@Composable
fun PlaylistMosaic(albumIds: List<Long>, size: Dp, modifier: Modifier = Modifier) {
    val shape = MaterialTheme.shapes.small
    val box = modifier.size(size).clip(shape)
    when {
        albumIds.size >= 4 -> Column(box) {
            albumIds.take(4).chunked(2).forEach { pair ->
                Row(Modifier.weight(1f)) {
                    pair.forEach { id ->
                        AsyncImage(Art.album(id), null, Modifier.weight(1f).aspectRatio(1f), contentScale = ContentScale.Crop)
                    }
                }
            }
        }
        albumIds.isNotEmpty() -> AsyncImage(Art.album(albumIds.first()), null, box, contentScale = ContentScale.Crop)
        else -> Icon(painterResource(R.drawable.koan_playlist), null, box.fillMaxSize())
    }
}

/** Ask for a playlist's name, for a new one or a rename. */
@Composable
fun NameDialog(title: String, confirm: String, initial: String = "", onDismiss: () -> Unit, onName: (String) -> Unit) {
    var name by remember { mutableStateOf(TextFieldValue(initial, TextRange(0, initial.length))) }
    val focus = remember { FocusRequester() }
    val done = { if (name.text.isNotBlank()) onName(name.text.trim()) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            OutlinedTextField(
                value = name,
                onValueChange = { name = it },
                singleLine = true,
                placeholder = { Text("Name") },
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences, imeAction = ImeAction.Done),
                keyboardActions = KeyboardActions(onDone = { done() }),
                modifier = Modifier.focusRequester(focus),
            )
        },
        confirmButton = { TextButton(onClick = done, enabled = name.text.isNotBlank()) { Text(confirm) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
    LaunchedEffect(Unit) { focus.requestFocus() }
}

/** Ask before something that cannot be undone. */
@Composable
fun ConfirmDialog(title: String, detail: String, confirm: String, onDismiss: () -> Unit, onConfirm: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = { Text(detail) },
        confirmButton = {
            TextButton(onClick = { onDismiss(); onConfirm() }) { Text(confirm, color = MaterialTheme.colorScheme.error) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/** "48 min", or "2 h 5 min": a playlist's or a day's length, to the minute. */
fun duration(ms: Long): String {
    val minutes = (ms + 30_000) / 60_000
    return if (minutes < 60) "$minutes min" else "${minutes / 60} h ${minutes % 60} min"
}
