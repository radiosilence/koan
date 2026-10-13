package cc.blit.koan.ui.queue

import androidx.compose.foundation.clickable
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.ListItem
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.ui.StubPage
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.navigation.Tab

fun EntryProviderScope<NavKey>.queueEntries() {
    entry<Tab.Queue> { QueuePage() }
}

@Composable
private fun QueuePage() {
    val app = koanApp()
    val queue by app.mirror.queue.collectAsStateWithLifecycle()
    if (queue.items.isEmpty()) return StubPage("Nothing queued")
    LazyColumn {
        items(queue.items, key = { it.queueItemId }) { row ->
            ListItem(
                headlineContent = { Text(row.title) },
                supportingContent = { Text("${row.artist} · ${row.status.name.lowercase()}") },
                modifier = Modifier.clickable { app.player.play(row.queueItemId) },
            )
        }
    }
}
