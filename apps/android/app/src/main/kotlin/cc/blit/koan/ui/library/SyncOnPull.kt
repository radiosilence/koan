package cc.blit.koan.ui.library

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.material3.pulltorefresh.PullToRefreshDefaults
import androidx.compose.material3.pulltorefresh.rememberPullToRefreshState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.ui.ActivityModel
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.theme.KoanTheme

/**
 * A library listing whose top, pulled down and let go, syncs with the server,
 * as Settings' Sync does. Offered only while this device has a server; the
 * indicator turns while any sync runs, whoever started it.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SyncOnPull(content: @Composable () -> Unit) {
    val app = koanApp()
    val connection by app.mirror.connection.collectAsStateWithLifecycle()
    val tasks by app.activity.tasks.collectAsStateWithLifecycle()
    val syncing = tasks.any { it.followsSync }
    val state = rememberPullToRefreshState()
    if (connection?.signedInTo == null) {
        content()
        return
    }
    PullToRefreshBox(
        isRefreshing = syncing,
        onRefresh = {
            if (!syncing) {
                app.mirror.launch {
                    app.activity.run(ActivityModel.SYNCING, followsSync = true) { syncRemote() }
                }
            }
        },
        modifier = Modifier.fillMaxSize(),
        state = state,
        indicator = {
            PullToRefreshDefaults.Indicator(
                state = state,
                isRefreshing = syncing,
                modifier = Modifier.align(Alignment.TopCenter),
                containerColor = KoanTheme.colors.surface,
                color = KoanTheme.look.accent,
            )
        },
    ) {
        Box(Modifier.fillMaxSize()) { content() }
    }
}
