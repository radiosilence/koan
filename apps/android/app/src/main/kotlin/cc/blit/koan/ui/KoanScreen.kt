package cc.blit.koan.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import cc.blit.koan.R
import cc.blit.koan.ffi.PlayState

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun KoanScreen(model: LibraryModel = viewModel()) {
    val state by model.state.collectAsStateWithLifecycle()
    val album = state.album
    BackHandler(enabled = album != null) { model.openAlbum(null) }
    Scaffold(
        topBar = {
            TopAppBar(title = { Text(album?.album?.title ?: "kōan", maxLines = 1, overflow = TextOverflow.Ellipsis) })
        },
        bottomBar = { NowPlayingBar(model) },
    ) { padding ->
        Box(Modifier.padding(padding).fillMaxSize()) {
            when {
                state.signedIn == false -> SignIn(state, model::signIn)
                album != null -> LazyColumn {
                    itemsIndexed(album.tracks, key = { _, t -> t.id }) { index, track ->
                        ListItem(
                            headlineContent = { Text(track.title) },
                            supportingContent = { Text(track.artistName) },
                            leadingContent = { Text(track.trackNumber?.toString() ?: "") },
                            modifier = Modifier.clickable { model.play(album.tracks.map { it.id }, index) },
                        )
                    }
                }
                else -> LazyColumn {
                    items(state.albums, key = { it.id }) { a ->
                        ListItem(
                            headlineContent = { Text(a.title) },
                            supportingContent = { Text(a.artistName) },
                            modifier = Modifier.clickable { model.openAlbum(a.id) },
                        )
                    }
                }
            }
            state.busy?.let { label ->
                Column(Modifier.align(Alignment.Center), horizontalAlignment = Alignment.CenterHorizontally) {
                    CircularProgressIndicator()
                    Text(label, Modifier.padding(top = 12.dp))
                }
            }
        }
    }
}

@Composable
private fun SignIn(state: LibraryState, onSignIn: (String, String, String) -> Unit) {
    var url by rememberSaveable { mutableStateOf("") }
    var username by rememberSaveable { mutableStateOf("") }
    var password by rememberSaveable { mutableStateOf("") }
    Column(Modifier.padding(16.dp).fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        OutlinedTextField(
            url,
            { url = it },
            label = { Text("Server") },
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
            modifier = Modifier.fillMaxWidth(),
        )
        OutlinedTextField(
            username,
            { username = it },
            label = { Text("User") },
            singleLine = true,
            modifier = Modifier.fillMaxWidth(),
        )
        OutlinedTextField(
            password,
            { password = it },
            label = { Text("Password") },
            singleLine = true,
            visualTransformation = PasswordVisualTransformation(),
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
            modifier = Modifier.fillMaxWidth(),
        )
        state.error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        Button(
            onClick = { onSignIn(url, username, password) },
            enabled = state.busy == null && url.isNotBlank() && username.isNotBlank(),
        ) { Text("Sign in") }
    }
}

@Composable
private fun NowPlayingBar(model: LibraryModel) {
    val now by model.nowPlaying.collectAsStateWithLifecycle()
    val playhead by model.playhead.collectAsStateWithLifecycle()
    val entry = now?.entry ?: return
    Surface(tonalElevation = 3.dp) {
        Column {
            HorizontalDivider()
            Row(
                Modifier.fillMaxWidth().navigationBarsPadding().padding(horizontal = 16.dp, vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(Modifier.weight(1f)) {
                    Text(entry.title, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(
                        "${entry.artist} · ${clock(playhead.positionMs)} / ${clock(now?.durationMs?.toLong() ?: 0)}",
                        style = MaterialTheme.typography.bodySmall,
                        maxLines = 1,
                    )
                }
                val playing = now?.state == PlayState.PLAYING
                IconButton(onClick = model::togglePlayPause) {
                    Icon(
                        painterResource(if (playing) R.drawable.ic_pause else R.drawable.ic_play),
                        contentDescription = if (playing) "Pause" else "Play",
                    )
                }
                IconButton(onClick = model::next) {
                    Icon(painterResource(R.drawable.ic_next), contentDescription = "Next")
                }
            }
        }
    }
}

private fun clock(ms: Long): String {
    val s = ms / 1000
    return "%d:%02d".format(s / 60, s % 60)
}
