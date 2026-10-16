package cc.blit.koan.ui

import androidx.compose.runtime.Composable
import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavEntry
import androidx.navigation3.runtime.entryProvider
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.nav.Route
import cc.blit.koan.nav.Section
import cc.blit.koan.ui.pages.AlbumPage
import cc.blit.koan.ui.pages.AlbumsPage
import cc.blit.koan.ui.pages.ArtistPage
import cc.blit.koan.ui.pages.ArtistsPage
import cc.blit.koan.ui.pages.DownloadsPage
import cc.blit.koan.ui.pages.FavouritesPage
import cc.blit.koan.ui.pages.LibraryPage
import cc.blit.koan.ui.pages.OnDevicePage
import cc.blit.koan.ui.pages.PlayHistoryPage
import cc.blit.koan.ui.pages.PlaylistPage
import cc.blit.koan.ui.pages.PlaylistsPage
import cc.blit.koan.ui.pages.QueuePage
import cc.blit.koan.ui.pages.RecentlyPlayedPage
import cc.blit.koan.ui.pages.SearchPage
import cc.blit.koan.ui.pages.SettingsPage
import cc.blit.koan.ui.pages.TracksPage

/**
 * **The registration point for pages.** Every [Route] is drawn by one function
 * in `ui/pages/`, named here and taking the [KoanRuntime]; this file is the
 * only place a route is bound to its page, and a page worker changes the body
 * of its own file and leaves this one alone.
 *
 * Contract for a page (the same as `EngineMirror`'s, repeated because it is
 * what keeps the UI thread free):
 *
 *  - Read state by collecting a flow from `runtime.mirror`, `runtime.library`,
 *    `runtime.playlists`, `runtime.activity` or `runtime.player` with
 *    `collectAsStateWithLifecycle`, so a backgrounded page stops collecting.
 *  - Never call `runtime.engine` from a composable or a list item. Call a model
 *    method; it returns at once and does the work on the engine dispatcher.
 *  - A page draws from the route that pushed it, never from `navigator.current`:
 *    a stack keeps every page it pushed.
 *  - Lists use stable keys (`QueueItemId`, album id) and a `contentType` on
 *    every item. Artwork goes through `Sleeve`, never a bitmap held in a model.
 *  - Colours, type, spacing and controls come from the theme (`KoanTheme`,
 *    `KoanText`, `KoanButton`, ...); no Material colours, shapes or fonts.
 */
fun EntryProviderScope<Route>.registerPages(runtime: KoanRuntime) {
    entry<Route.Library> { LibraryPage(runtime) }
    entry<Route.Settings> { SettingsPage(runtime) }
    entry<Route.Playlists> { PlaylistsPage(runtime) }
    entry<Route.Album> { AlbumPage(runtime, it.id) }
    entry<Route.Artist> { ArtistPage(runtime, it.id) }
    entry<Route.Page> { SectionPage(runtime, it.section) }
}

/** A section is bound to its page here, as the Swift `SectionPage` does. */
@Composable
fun SectionPage(runtime: KoanRuntime, section: Section) {
    when (section) {
        Section.Queue -> QueuePage(runtime)
        Section.SearchResults -> SearchPage(runtime)
        Section.Albums -> AlbumsPage(runtime)
        Section.Artists -> ArtistsPage(runtime)
        Section.Tracks -> TracksPage(runtime)
        Section.Favourites -> FavouritesPage(runtime)
        Section.RecentlyPlayed -> RecentlyPlayedPage(runtime)
        Section.OnDevice -> OnDevicePage(runtime)
        Section.PlayHistory -> PlayHistoryPage(runtime)
        Section.Downloads -> DownloadsPage(runtime)
        is Section.Playlist -> PlaylistPage(runtime, section.id)
    }
}

/** The entry provider the nav display is given. */
fun appEntries(runtime: KoanRuntime): (Route) -> NavEntry<Route> = entryProvider { registerPages(runtime) }
