package cc.blit.koan.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.material3.Scaffold
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.navigation3.ui.NavDisplay
import cc.blit.koan.engine.KoanProcess
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.nav.Tab
import cc.blit.koan.state.ServerStatus
import cc.blit.koan.state.SettingsModel
import cc.blit.koan.theme.KoanAccent
import cc.blit.koan.theme.KoanRole
import cc.blit.koan.theme.KoanTab
import cc.blit.koan.theme.KoanTabBar
import cc.blit.koan.theme.KoanTheme
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanTone
import cc.blit.koan.ui.pages.MiniPlayer
import cc.blit.koan.ui.pages.SignInPage

/**
 * The app: the theme, then whichever of three things the engine allows. Not up
 * yet (or failed) says so; signed out shows the sign-in page in place of the
 * tabs; otherwise the tabs.
 *
 * The record playing sets the accent, so it is read here and handed to the
 * theme: everything below draws in it.
 */
@Composable
fun AppShell() {
    val runtime by KoanProcess.runtime.collectAsStateWithLifecycle()
    val failure by KoanProcess.failure.collectAsStateWithLifecycle()
    val ready = runtime
    val accent by (ready?.accent ?: remember { kotlinx.coroutines.flow.MutableStateFlow(KoanAccent.Mint) })
        .collectAsStateWithLifecycle()
    KoanTheme(accent = accent) {
        Box(Modifier.fillMaxSize().background(KoanTheme.colors.bg)) {
            when {
                failure != null -> Message("the engine did not start: $failure", Modifier.safeDrawingPadding())
                ready == null -> Message("starting the engine", Modifier.safeDrawingPadding())
                else -> Ready(ready)
            }
        }
    }
}

@Composable
private fun Message(text: String, modifier: Modifier = Modifier) {
    Box(modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        KoanText(text, KoanRole.Body, KoanTone.Muted, maxLines = 6)
    }
}

@Composable
private fun Ready(runtime: KoanRuntime) {
    val settings by runtime.settings.settings.collectAsStateWithLifecycle()
    val connection by runtime.mirror.connection.collectAsStateWithLifecycle()
    val status = SettingsModel.status(settings, connection?.signInRefused == true, connection?.linked == true)
    when (status) {
        ServerStatus.SignedOut, is ServerStatus.Refused -> SignInPage(runtime)
        // Loading shows the tabs too: a signed-in phone never flashes the sign-in page on launch.
        else -> Tabs(runtime)
    }
}

private val TabItems = Tab.entries.map { KoanTab(it, it.label) }

/**
 * Four tabs over a stack each. The back stack drawn is the one the navigator
 * holds for the tab in front: Navigation 3 owns no state of its own here.
 */
@Composable
private fun Tabs(runtime: KoanRuntime) {
    val navigator = runtime.navigator
    val nav by navigator.state.collectAsStateWithLifecycle()
    // At a non-start tab's root Back goes to the Queue tab; NavDisplay handles
    // Back inside a stack.
    BackHandler(enabled = !nav.canPop && nav.tab != Tab.Queue) { navigator.back() }
    val entries = remember(runtime) { appEntries(runtime) }
    Scaffold(
        containerColor = KoanTheme.colors.bg,
        bottomBar = {
            Column(Modifier.navigationBarsPadding()) {
                MiniPlayer(runtime)
                KoanTabBar(TabItems, nav.tab, onSelect = navigator::selectTab, onReselect = navigator::selectTab)
            }
        },
    ) { insets ->
        NavDisplay(
            backStack = nav.stack,
            onBack = { navigator.back() },
            entryProvider = entries,
            modifier = Modifier.padding(insets),
        )
    }
}
