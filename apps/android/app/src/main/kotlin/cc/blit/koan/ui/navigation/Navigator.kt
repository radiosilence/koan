package cc.blit.koan.ui.navigation

import androidx.compose.runtime.MutableIntState
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.navigation3.runtime.NavBackStack
import androidx.navigation3.runtime.NavEntry
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.ui.nowplaying.NowPlayingRoute
import java.util.UUID
import kotlinx.serialization.Serializable

/** The tabs, as on the phone: each keeps its own back stack while another is shown. */
@Serializable
sealed interface Tab : NavKey {
    @Serializable data object Queue : Tab

    @Serializable data object Library : Tab

    @Serializable data object Search : Tab

    @Serializable data object Settings : Tab

    companion object {
        val all = listOf(Queue, Library, Search, Settings)
    }
}

/** The root of the app's own stack: the tabs, under anything shown over all of them. */
@Serializable
data object Shell : NavKey

/**
 * A fresh id for a pushed page's key. Each tab keeps its pages while another is
 * shown, so the same page can be open in two tabs at once, and Navigation 3
 * tells entries apart by key: equal pages pushed twice must still be two.
 */
fun nonce() = UUID.randomUUID().toString()

/**
 * Where the app is, and the way to move. Pages read it from `LocalNavigator`.
 *
 * `push` and `pop` act on the open tab's stack. `openNowPlaying` covers the
 * tabs, from any of them.
 */
class Navigator internal constructor(
    private val stacks: Map<Tab, NavBackStack<NavKey>>,
    /** The open tab's place in `Tab.all`, saved as a number since a key is not saveable. */
    private val selected: MutableIntState,
    private val root: NavBackStack<NavKey>,
) {
    val tab: Tab get() = Tab.all[selected.intValue]

    /** How many times each tab's first page has been made again: see `select`. */
    private val resets = mutableStateMapOf<Tab, Int>()

    /** The count `tab`'s first page is keyed on, so a new count makes it again, at its top. */
    fun reset(tab: Tab) = resets[tab] ?: 0

    /**
     * Open `tab`. Choosing the open tab again returns it to its first page,
     * and at its first page, to the top, as a tab bar does: the page is made
     * again, its scroll and other kept state with it, and what it lists stays
     * in its model.
     */
    fun select(tab: Tab) {
        if (tab == this.tab) {
            val stack = stacks.getValue(tab)
            if (stack.size > 1) {
                while (stack.size > 1) stack.removeAt(stack.lastIndex)
            } else {
                resets[tab] = reset(tab) + 1
            }
        } else {
            selected.intValue = Tab.all.indexOf(tab)
        }
    }

    /** Open `key` in the open tab. Now Playing gets out of the way of a page opened from it. */
    fun push(key: NavKey) {
        closeNowPlaying()
        stacks.getValue(tab).add(key)
    }

    fun pop() {
        val stack = stacks.getValue(tab)
        if (stack.size > 1) stack.removeAt(stack.lastIndex)
    }

    fun openNowPlaying() {
        if (root.lastOrNull() !is NowPlayingRoute) root.add(NowPlayingRoute)
    }

    fun closeNowPlaying() {
        if (root.lastOrNull() is NowPlayingRoute) root.removeAt(root.lastIndex)
    }
}

/**
 * `entries` with each tab's first page keyed on its reset count: the count
 * moving makes the page again, with none of the saved state it had, as iOS
 * remakes a tab's stack when its tab is chosen again at the root. The key sits
 * inside the entry, under every decorator, so its saved state goes with it.
 */
fun Navigator.resetting(entries: (NavKey) -> NavEntry<NavKey>): (NavKey) -> NavEntry<NavKey> = { route ->
    val entry = entries(route)
    if (route !is Tab) entry else NavEntry<NavKey>(entry) { _ -> key(reset(route)) { entry.Content() } }
}

val LocalNavigator = staticCompositionLocalOf<Navigator> { error("no Navigator: compose inside AppShell") }
