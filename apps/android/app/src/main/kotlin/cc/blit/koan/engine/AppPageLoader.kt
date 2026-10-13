package cc.blit.koan.engine

import cc.blit.koan.nav.PageLoader
import cc.blit.koan.nav.Route
import cc.blit.koan.nav.Section
import cc.blit.koan.state.LibraryModel
import cc.blit.koan.state.PlaylistsModel

/**
 * What the navigator reads before it moves to a page: the library's listing or
 * record, and for a playlist its rows first (`Navigator.prepared(for:)` in
 * Swift). A playlist is a page like any other, and its rows are read before
 * the move like any other page's; the room takes its colour from its first
 * record, so that sleeve is warmed.
 */
class AppPageLoader(
    private val library: LibraryModel,
    private val playlists: PlaylistsModel,
) : PageLoader {
    override suspend fun prepare(route: Route): Any? {
        if (route is Route.Page && route.section is Section.Playlist) {
            val id = route.section.id
            playlists.prepare(id)
            playlists.covers.value[id]?.firstOrNull()?.let(library::warm)
        }
        return library.prepare(route)
    }

    override fun arrive(route: Route, prepared: Any?) = library.arrive(route, prepared)
}
