package cc.blit.koan.nav

/**
 * A row of the library's own listings, and the queue and search: what the
 * Mac's sidebar lists, and what the library browses by. The port of
 * `Navigator.Section`.
 */
sealed interface Section {
    data object Queue : Section
    data object SearchResults : Section
    data object Albums : Section
    data object Artists : Section
    data object Tracks : Section
    data object Favourites : Section
    data object RecentlyPlayed : Section
    data object OnDevice : Section
    data object PlayHistory : Section
    data object Downloads : Section

    /** One playlist. A page like any other. */
    data class Playlist(val id: Long) : Section

    /** The library's own listings, which the browse filters and a shelf's headings narrow. */
    val isBrowser: Boolean get() = this == Albums || this == Artists || this == Tracks

    /**
     * What the filter field says, and by its absence which sections have no
     * filter at all. A shelf shows the first few of each kind, and narrowing is
     * what its headings open: the browser, filtered to the shelf.
     */
    val filterPlaceholder: String?
        get() = when (this) {
            Albums -> "Filter albums"
            Artists -> "Filter artists"
            Tracks -> "Filter tracks"
            PlayHistory -> "Filter history"
            else -> null
        }

    companion object {
        /** The sections the Library tab lists, in the order it lists them. */
        val library: List<Section> = listOf(Albums, Artists, Tracks, Favourites, RecentlyPlayed, OnDevice, PlayHistory, Downloads)
    }
}

/**
 * Somewhere a tab's stack can go. Everywhere you can be, in one value: pages
 * draw from the route that pushed them, never from where the navigator is.
 *
 * **The registration point for pages** is `ui/AppPages.kt`: each route type
 * has one entry registered there, and a page reads its state from the
 * `PageEnv` it is given.
 */
sealed interface Route {
    /** The Library tab's own page: the list of sections and playlists. */
    data object Library : Route

    /** The Settings tab's own page. */
    data object Settings : Route

    /** The list of playlists. The Mac has no page for it, because its sidebar is that list. */
    data object Playlists : Route

    /** A section of the library, the queue, search results or one playlist. */
    data class Page(val section: Section) : Route

    data class Album(val id: Long) : Route

    data class Artist(val id: Long) : Route

    companion object {
        val Queue: Route = Page(Section.Queue)
        val Search: Route = Page(Section.SearchResults)
        fun playlist(id: Long): Route = Page(Section.Playlist(id))
    }
}

/**
 * The four tabs. Four, deliberately: five is where a bottom bar stops
 * fitting, and each is a stack of its own that keeps its place while another
 * is in front.
 */
enum class Tab(val label: String, val root: Route) {
    Queue("Queue", Route.Queue),
    Library("Library", Route.Library),
    Search("Search", Route.Search),
    Settings("Settings", Route.Settings),
}
