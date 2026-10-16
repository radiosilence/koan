package cc.blit.koan.ui.library

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.material3.AlertDialog
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.KoanApp
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.navigation.Navigator
import cc.blit.koan.ui.navigation.Tab
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanType

/**
 * Plays `playable` in place of the queue and goes where the music can be
 * seen: a record opens, since its tracks are on its page; an artist's page is
 * a wall of sleeves with nothing on it to show what started, so an artist goes
 * to the queue.
 */
fun KoanApp.playNow(nav: Navigator, playable: Playable) {
    when (playable) {
        is Playable.OfAlbum -> nav.push(AlbumRoute(playable.album.id))
        is Playable.OfArtist -> nav.select(Tab.Queue)
        is Playable.OfTrack -> Unit
    }
    play(playable)
}

/**
 * A tap that plays `playable` in place of the queue: a sleeve, an artist's
 * play button. A queue built up over an evening should not go to a tap meant
 * to open something, so with anything queued it asks first.
 */
@Composable
fun rememberPlayNow(playable: Playable): () -> Unit {
    val app = koanApp()
    val nav = LocalNavigator.current
    var asking by remember { mutableStateOf(false) }
    if (asking) ReplaceQueueDialog(playable) { asking = false }
    return { if (app.mirror.queue.value.items.isEmpty()) app.playNow(nav, playable) else asking = true }
}

/** Replace the queue with `playable`, add it to the end instead, or leave it. */
@Composable
fun ReplaceQueueDialog(playable: Playable, onDismiss: () -> Unit) {
    val app = koanApp()
    val nav = LocalNavigator.current
    val queued by app.mirror.queue.collectAsStateWithLifecycle()
    val count = queued.items.size
    val what = when (playable) {
        is Playable.OfAlbum -> "this record"
        is Playable.OfArtist -> "this artist"
        is Playable.OfTrack -> "this track"
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { KoanText("Replace the queue?", role = KoanType.TitleSmall, appText = true) },
        text = { KoanText("Playing $what takes the ${count(count.toLong(), "track")} queued off.") },
        confirmButton = {
            KoanButton("Play", {
                onDismiss()
                app.playNow(nav, playable)
            })
        },
        dismissButton = {
            Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.m)) {
                KoanButton("Cancel", onDismiss)
                KoanButton("Add to Queue", {
                    onDismiss()
                    app.mirror.launch { app.player.enqueue(playable.trackIds(this)) }
                })
            }
        },
    )
}
