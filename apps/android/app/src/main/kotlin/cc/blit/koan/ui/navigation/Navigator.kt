package cc.blit.koan.ui.navigation

import androidx.compose.runtime.MutableIntState
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.navigation3.runtime.NavBackStack
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

    /** Open `tab`; choosing the open tab again returns it to its first page. */
    fun select(tab: Tab) {
        if (tab == this.tab) {
            val stack = stacks.getValue(tab)
            while (stack.size > 1) stack.removeAt(stack.lastIndex)
        } else {
            selected.intValue = Tab.all.indexOf(tab)
        }
    }

    fun push(key: NavKey) {
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

val LocalNavigator = staticCompositionLocalOf<Navigator> { error("no Navigator: compose inside AppShell") }
