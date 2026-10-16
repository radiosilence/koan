package cc.blit.koan.nav

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull

/**
 * What the stacks hold now. Immutable: a collector of it is woken by a move
 * and by nothing else.
 */
data class NavState(
    val tab: Tab,
    /** Each tab's stack, root first. Never empty: a tab is at least its own page. */
    val stacks: Map<Tab, List<Route>>,
) {
    /** The tab in front's stack: what the nav display draws. */
    val stack: List<Route> get() = stacks.getValue(tab)

    /** The page on top of the tab in front. */
    val current: Route get() = stack.last()

    /** Whether Back has somewhere to go inside the tab in front. */
    val canPop: Boolean get() = stack.size > 1

    /** The page on top of a tab. */
    fun top(of: Tab): Route = stacks.getValue(of).last()

    companion object {
        val Start = NavState(Tab.Queue, Tab.entries.associateWith { listOf(it.root) })
    }
}

/**
 * The seam between where the app is and what the models are showing. The
 * library's listing is loaded by moving it, so a page that did not move it
 * would draw the last one's rows.
 */
interface PageLoader {
    /**
     * Everything the page draws, in hand before it is shown. Whatever this
     * returns is handed back to [arrive] when the move lands, and may be null.
     */
    suspend fun prepare(route: Route): Any?

    /** The page is on screen: adopt what [prepare] read. Runs in step with the stacks changing. */
    fun arrive(route: Route, prepared: Any?)
}

/**
 * The one owner of where you are. The port of `Navigator.swift` onto the
 * per-tab stacks the iOS app uses (`TabShell`'s `paths`): four tabs, each a
 * stack of [Route]s that keeps its place while another is in front.
 *
 * The back stack is plain state this class owns, which Navigation 3 draws:
 * nothing is derived from a path, so there is no write to guard against. An
 * album is reachable from the queue, from the grid, from an artist, from
 * search, and none of those is its parent: a record is pushed on whichever
 * tab it was opened from.
 *
 * **Loaded, then moved.** A page's rows are read *before* it is shown. An
 * arrival followed by a fetch draws a frame of the word "Album" over an empty
 * list, and then the real page flickering in underneath it. One move at a
 * time: a second tap while the first page is still being read wins, and the
 * older move is cancelled before it can apply. `destination` says where the
 * current move is going, for a tab bar that lights it at once.
 *
 * **Back.** Pops the tab in front. At a tab's root, Back goes to the Queue
 * tab (the app's start), and at the Queue root it returns false so the system
 * may leave the app, which is the platform's convention for a bottom bar.
 * Tapping the tab already in front goes back to its root, and at the root
 * bumps [rewinds] so a list can scroll itself to the top.
 *
 * [scope] must run its work off Main (the loader reads the engine).
 */
class Navigator(
    private val scope: CoroutineScope,
    private val loader: PageLoader = object : PageLoader {
        override suspend fun prepare(route: Route): Any? = null
        override fun arrive(route: Route, prepared: Any?) = Unit
    },
) {
    private val _state = MutableStateFlow(NavState.Start)
    private val _destination = MutableStateFlow<Route?>(null)
    private val _highlightedTrackId = MutableStateFlow<Long?>(null)
    private val _rewinds = MutableStateFlow<Map<Tab, Int>>(emptyMap())
    private var moving: Job? = null

    val state: StateFlow<NavState> = _state.asStateFlow()

    /** Where the move being loaded is going, if there is one. */
    val destination: StateFlow<Route?> = _destination.asStateFlow()

    /**
     * The track a move was aimed at, so the album page can single it out
     * rather than dropping you at the top of a twenty-track record. Cleared by
     * the page once it has scrolled to it.
     */
    val highlightedTrackId: StateFlow<Long?> = _highlightedTrackId.asStateFlow()

    /**
     * How many times each tab has been sent back to its top. A list keyed on
     * its tab's count scrolls to the top when it moves.
     */
    val rewinds: StateFlow<Map<Tab, Int>> = _rewinds.asStateFlow()

    val current: Route get() = _state.value.current

    // MARK: - Moves

    /** Show a section on the tab in front. */
    fun show(section: Section) = go(Route.Page(section))

    fun openAlbum(id: Long, highlighting: Long? = null) {
        // Set before the move rather than on arrival: the page that reads it
        // is not on screen yet, and this is the same tap.
        _highlightedTrackId.value = highlighting
        go(Route.Album(id))
    }

    fun openArtist(id: Long) = go(Route.Artist(id))

    fun openPlaylist(id: Long) = go(Route.playlist(id))

    fun clearHighlight() {
        _highlightedTrackId.value = null
    }

    /**
     * Go to a page, recording it: the only way anything moves forward. A
     * tab's own page brings that tab forward, back at its root; anything else
     * is pushed on the tab in front. The page already on top is not pushed
     * again, and a move to it cancels the one still loading.
     */
    @Synchronized
    fun go(route: Route) {
        val state = _state.value
        val owner = Tab.entries.firstOrNull { it.root == route }
        if (owner != null) {
            move(route) { it.copy(tab = owner, stacks = it.stacks + (owner to listOf(owner.root))) }
            return
        }
        if (state.current == route) {
            moving?.cancel()
            _destination.value = null
            return
        }
        move(route) { it.copy(stacks = it.stacks + (it.tab to (it.stack + route))) }
    }

    /**
     * Bring a tab forward, where it was. Chosen again while in front: back to
     * its root, and at the root, to the top, as a tab bar does.
     */
    @Synchronized
    fun selectTab(tab: Tab) {
        val state = _state.value
        if (tab != state.tab) {
            val target = state.top(of = tab)
            move(target) { it.copy(tab = tab) }
            return
        }
        if (state.canPop) {
            move(tab.root) { it.copy(stacks = it.stacks + (tab to listOf(tab.root))) }
        } else {
            moving?.cancel()
            _destination.value = null
            _rewinds.value = _rewinds.value + (tab to (_rewinds.value[tab] ?: 0) + 1)
        }
    }

    /**
     * The system Back. True if it moved; false at the Queue root, where the
     * app has nowhere left to go and the system should take over.
     */
    @Synchronized
    fun back(): Boolean {
        val state = _state.value
        return when {
            state.canPop -> {
                val shorter = state.stack.dropLast(1)
                move(shorter.last()) { it.copy(stacks = it.stacks + (it.tab to shorter)) }
                true
            }
            state.tab != Tab.Queue -> {
                move(state.top(of = Tab.Queue)) { it.copy(tab = Tab.Queue) }
                true
            }
            else -> false
        }
    }

    /**
     * Drop a page from every stack. Search results only exist while there is
     * a query, and a deleted playlist no longer exists at all; once either is
     * gone, they are not somewhere Back can return to. A tab's root stays.
     */
    @Synchronized
    fun forget(section: Section) {
        val gone = Route.Page(section)
        val before = _state.value
        val stacks = before.stacks.mapValues { (tab, stack) ->
            val kept = stack.filter { it != gone || it == tab.root }
            kept.ifEmpty { listOf(tab.root) }
        }
        if (stacks == before.stacks) return
        _state.value = before.copy(stacks = stacks)
        val now = _state.value.current
        if (now != before.current) follow(now)
    }

    /**
     * Show the Queue tab once it holds what was just started. Playing
     * something is not a reason to move: you stay where you were, and the
     * queue keeps running behind you until you go to it yourself.
     *
     * Queue mutations run off Main, so switching immediately shows the old
     * queue for a frame or two and then flickers. Waiting for the engine to
     * confirm avoids that, but only briefly: if the mutation is slow enough to
     * notice, jumping to the queue after the fact would feel like the app
     * moving on its own, so it stays put instead.
     */
    fun showQueueWhenReady(version: StateFlow<Long>, waitMs: Long = 50) {
        val before = version.value
        scope.launch {
            val changed = withTimeoutOrNull(waitMs) { version.first { it != before } } != null
            if (changed) selectTab(Tab.Queue)
        }
    }

    // MARK: - Loading

    /**
     * Load the page, *then* move to it. One task at a time, and the stacks
     * change in the same step as the loader adopts what it read, so the page
     * and its rows are one change.
     */
    private fun move(route: Route, apply: (NavState) -> NavState) {
        moving?.cancel()
        _destination.value = route
        moving = scope.launch {
            val prepared = try {
                loader.prepare(route)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                null
            }
            // A cancelled move never applies: this check is the "latest tap wins".
            ensureActive()
            _state.update(apply)
            loader.arrive(route, prepared)
            _destination.value = null
        }
    }

    /** Bring the models to what the stacks now show, without moving them. */
    private fun follow(route: Route) {
        moving?.cancel()
        moving = scope.launch {
            val prepared = try {
                loader.prepare(route)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                null
            }
            ensureActive()
            if (_state.value.current == route) loader.arrive(route, prepared)
        }
    }
}
