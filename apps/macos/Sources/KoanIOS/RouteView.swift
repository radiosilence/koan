import KoanFFI
import SwiftUI

/// Somewhere a tab's stack can go.
///
/// Mostly a navigator page. The list of playlists is the exception: the Mac has
/// no page for it, because its sidebar is that list.
enum Route: Hashable {
    case page(Navigator.Page)
    case playlists

    var page: Navigator.Page? {
        if case .page(let page) = self { page } else { nil }
    }
}

/// A page, drawn from the route that pushed it rather than from wherever the
/// navigator is.
///
/// A stack keeps every page it pushed and draws them all during a swipe back,
/// so a page that read `nav.current` would show the page above it as it went.
/// The navigator still follows along — see `TabShell` — because the library's
/// listing is loaded by moving it.
struct RouteView: View {
    let route: Route

    var body: some View {
        content
            .washedGround()
            .roomBackground()
    }

    @ViewBuilder private var content: some View {
        switch route {
        case .playlists:
            PlaylistsList()
        case .page(.section(let section)):
            SectionPage(section: section)
        case .page(.album(let id)):
            AlbumDetailView(albumId: id)
        case .page(.artist(let id)):
            ArtistDetailView(artistId: id)
        }
    }
}

private struct SectionPage: View {
    let section: Navigator.Section

    var body: some View {
        page
            .navigationTitle(title)
            .modifier(SectionFilter(placeholder: section.filterPlaceholder))
            #if os(tvOS)
            // In the page rather than the navigation bar: a television's
            // toolbar takes focus, but a sheet or menu opened from it never
            // appears.
            .safeAreaInset(edge: .top) {
                if section.isBrowser {
                    BrowseControlsRow(section: section)
                }
            }
            #else
            .toolbar {
                if section.isBrowser {
                    ToolbarItem(placement: .topBarTrailing) {
                        BrowseFilterButton()
                    }
                }
                if section == .albums {
                    AlbumSortControls()
                }
                if section == .tracks {
                    TrackSortControls()
                }
            }
            #endif
    }

    @ViewBuilder private var page: some View {
        switch section {
        case .queue: QueueView()
        case .searchResults: SearchResultsView()
        case .albums: AlbumBrowser()
        case .artists: ArtistBrowser()
        case .tracks: TrackBrowser()
        case .favourites: FavouritesView()
        case .recentlyPlayed: RecentlyPlayedView()
        case .onDevice: OnDeviceView()
        case .playHistory: HistoryView()
        case .downloads: DownloadsView()
        case .playlist(let id): PlaylistView(playlistId: id)
        }
    }

    private var title: String {
        switch section {
        case .queue: "Queue"
        case .searchResults: "Search"
        case .albums: "Albums"
        case .artists: "Artists"
        case .tracks: "Tracks"
        case .favourites: "Favourites"
        case .recentlyPlayed: "Recently Played"
        case .onDevice: "Downloaded"
        case .playHistory: "History"
        case .downloads: "Downloads"
        case .playlist: ""
        }
    }
}

/// The Mac's toolbar filter, as the search field iOS puts under a page's title.
/// Its own modifier so that typing re-runs this and not the page.
private struct SectionFilter: ViewModifier {
    let placeholder: String?
    @Environment(LibraryModel.self) private var library

    func body(content: Content) -> some View {
        #if os(tvOS)
        // A search field on tvOS is a keyboard across the top of the page,
        // over its title and buttons. The Search tab finds things there.
        content
        #else
        if let placeholder {
            @Bindable var library = library
            content.searchable(text: $library.filter, prompt: placeholder)
        } else {
            content
        }
        #endif
    }
}

/// The track browser's sort: the choices, ticked, under one control.
private struct TrackSortMenu: View {
    @Environment(LibraryModel.self) private var library

    var body: some View {
        Menu {
            Picker("Sort", selection: Binding(
                get: { library.trackSort },
                set: { library.trackSort = $0 }
            )) {
                ForEach(TrackBrowseSort.offered(recent: library.browseFilter.recent), id: \.self) { sort in
                    Text(sort.label).tag(sort)
                }
            }
        } label: {
            Label("Sort", systemImage: "arrow.up.arrow.down")
        }
    }
}

/// The album browser's sort, as `TrackSortMenu` is the track browser's.
private struct AlbumSortMenu: View {
    @Environment(LibraryModel.self) private var library

    var body: some View {
        Menu {
            Picker("Sort", selection: Binding(
                get: { library.albumSort },
                set: { library.albumSort = $0 }
            )) {
                ForEach(AlbumSort.offered(
                    recent: library.browseFilter.recent, downloaded: library.browseFilter.downloaded
                ), id: \.self) { sort in
                    Text(sort.label).tag(sort)
                }
            }
        } label: {
            Label("Sort", systemImage: "arrow.up.arrow.down")
        }
    }
}

#if os(tvOS)
/// A browser's filters and sort on a television: a row of buttons above the
/// listing, reached by moving up from it, with room for their names.
private struct BrowseControlsRow: View {
    let section: Navigator.Section
    @Environment(LibraryModel.self) private var library

    var body: some View {
        HStack(spacing: 24) {
            Spacer()
            if section == .albums, library.albumSort == .random {
                Button { library.reshuffleAlbums() } label: {
                    Label("Shuffle", systemImage: Icon.reshuffle)
                }
            }
            BrowseFilterButton()
            if section == .albums {
                AlbumSortMenu()
            }
            if section == .tracks {
                TrackSortMenu()
            }
        }
        .padding(.horizontal, 80)
        .padding(.bottom, 16)
        .focusSection()
    }
}
#else
/// The track browser's sort, in the navigation bar.
private struct TrackSortControls: ToolbarContent {
    var body: some ToolbarContent {
        ToolbarItem(placement: .topBarTrailing) { TrackSortMenu() }
    }
}

/// The Mac's album sort, in the navigation bar. Reshuffle is its own button
/// for the same reason as there: it is pressed repeatedly.
private struct AlbumSortControls: ToolbarContent {
    @Environment(LibraryModel.self) private var library

    var body: some ToolbarContent {
        if library.albumSort == .random {
            ToolbarItem(placement: .topBarTrailing) {
                Button {
                    library.reshuffleAlbums()
                } label: {
                    Label("Shuffle", systemImage: Icon.reshuffle)
                }
            }
        }
        ToolbarItem(placement: .topBarTrailing) { AlbumSortMenu() }
    }
}
#endif
