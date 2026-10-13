package cc.blit.koan.ui

import android.widget.Toast
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Scaffold
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.navigation3.rememberViewModelStoreNavEntryDecorator
import androidx.navigation3.runtime.NavKey
import androidx.navigation3.runtime.entryProvider
import androidx.navigation3.runtime.rememberNavBackStack
import androidx.navigation3.runtime.rememberSaveableStateHolderNavEntryDecorator
import androidx.navigation3.ui.NavDisplay
import cc.blit.koan.ui.downloads.downloadsEntries
import cc.blit.koan.ui.history.historyEntries
import cc.blit.koan.ui.library.libraryEntries
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.navigation.Navigator
import cc.blit.koan.ui.navigation.Shell
import cc.blit.koan.ui.navigation.Tab
import cc.blit.koan.ui.nowplaying.MiniPlayer
import cc.blit.koan.ui.nowplaying.nowPlayingEntries
import cc.blit.koan.ui.playlists.playlistsEntries
import cc.blit.koan.ui.queue.queueEntries
import cc.blit.koan.ui.search.searchEntries
import cc.blit.koan.ui.settings.AccountPrompts
import cc.blit.koan.ui.settings.ControlledStrip
import cc.blit.koan.ui.settings.DspPrompts
import cc.blit.koan.ui.settings.AskForNotificationsOnFirstPlay
import cc.blit.koan.ui.settings.OpenedLinks
import cc.blit.koan.ui.settings.SignInPage
import cc.blit.koan.ui.settings.accountModel
import cc.blit.koan.ui.settings.dspModel
import cc.blit.koan.ui.settings.settingsEntries
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.recordWash
import cc.blit.koan.ui.theme.KoanTabBar
import cc.blit.koan.ui.theme.KoanTabItem

/**
 * The app's frame: sign-in until a server is signed in to, then the tabs, each
 * with its own back stack, the mini player above them, and Now Playing over
 * everything.
 *
 * Every package registers its pages here through its `…Entries()` function;
 * adding a page is a key and an `entry` in the package that owns it.
 */
@Composable
fun AppShell() {
    val account = accountModel()
    val state by account.state.collectAsStateWithLifecycle()
    when (state.signedIn) {
        null -> Unit
        false -> SignInPage(account)
        true -> Signed()
    }
    val dsp = dspModel()
    OpenedLinks(account, dsp)
    AccountPrompts(account)
    DspPrompts(dsp)
    AskForNotificationsOnFirstPlay()
    Notices()
}

/** What a command came to when it did not simply work, said once. */
@Composable
private fun Notices() {
    val app = koanApp()
    val context = LocalContext.current
    LaunchedEffect(app) {
        app.mirror.notices.collect { Toast.makeText(context, it, Toast.LENGTH_LONG).show() }
    }
}

@Composable
private fun Signed() {
    val root = rememberNavBackStack(Shell)
    val stacks = Tab.all.associateWith { rememberNavBackStack(it) }
    val selected = rememberSaveable { mutableIntStateOf(Tab.all.indexOf(Tab.Library)) }
    val navigator = remember(stacks, root) { Navigator(stacks, selected, root) }
    CompositionLocalProvider(LocalNavigator provides navigator) {
        // Every page sits on the record's wash: pages leave their ground bare.
        NavDisplay(
            modifier = Modifier.fillMaxSize().recordWash(),
            backStack = root,
            onBack = { navigator.closeNowPlaying() },
            entryDecorators = listOf(rememberSaveableStateHolderNavEntryDecorator(), rememberViewModelStoreNavEntryDecorator()),
            entryProvider = entryProvider {
                entry<Shell> { Tabs(navigator, stacks) }
                nowPlayingEntries()
            },
        )
    }
}

@Composable
private fun Tabs(navigator: Navigator, stacks: Map<Tab, List<NavKey>>) {
    val current = navigator.tab
    val stack = stacks.getValue(current)
    Scaffold(
        containerColor = Color.Transparent,
        bottomBar = {
            Column {
                ControlledStrip()
                MiniPlayer()
                KoanTabBar {
                    Tab.all.forEach { tab ->
                        KoanTabItem(tab.label, selected = tab == current, onClick = { navigator.select(tab) }, icon = tab.icon)
                    }
                }
            }
        },
    ) { padding ->
        Box(Modifier.padding(padding).consumeWindowInsets(padding).fillMaxSize()) {
            // The other tabs' stacks go beneath the open one's, so their pages
            // keep their saved state while hidden.
            val shown = Tab.all.filter { it != current }.flatMap { stacks.getValue(it) } + stack
            NavDisplay(
                backStack = shown,
                onBack = { navigator.pop() },
                entryDecorators = listOf(rememberSaveableStateHolderNavEntryDecorator(), rememberViewModelStoreNavEntryDecorator()),
                entryProvider = entryProvider {
                    queueEntries()
                    libraryEntries()
                    searchEntries()
                    settingsEntries()
                    playlistsEntries()
                    downloadsEntries()
                    historyEntries()
                },
            )
            // After NavDisplay, so it takes back first at a tab's root rather
            // than revealing the tab kept beneath.
            BackHandler(enabled = stack.size == 1 && current != Tab.Library) { navigator.select(Tab.Library) }
        }
    }
}

private val Tab.label
    get() = when (this) {
        Tab.Queue -> "Queue"
        Tab.Library -> "Library"
        Tab.Search -> "Search"
        Tab.Settings -> "Settings"
    }

private val Tab.icon
    get() = when (this) {
        Tab.Queue -> KoanIcons.queueSection
        Tab.Library -> KoanIcons.library
        Tab.Search -> KoanIcons.search
        Tab.Settings -> KoanIcons.settings
    }
