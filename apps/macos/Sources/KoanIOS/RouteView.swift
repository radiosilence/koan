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
        #if os(tvOS)
        // In the page rather than the navigation bar: a television's toolbar
        // takes focus, but a sheet or menu opened from it never appears. Above
        // the listing rather than inset over it, which would scroll beneath.
        VStack(spacing: 0) {
            if section.isBrowser || section.filterPlaceholder != nil {
                BrowseControlsRow(section: section)
            }
            page
        }
        .navigationTitle(KoanTheme.label(title))
        #else
        page
            .navigationTitle(KoanTheme.label(title))
            .modifier(SectionFilter(placeholder: section.filterPlaceholder))
            .toolbar {
                if section.isBrowser {
                    ToolbarItem(placement: .topBarTrailing) {
                        BrowseFilterButton().koanControl()
                    }
                    .sharedBackgroundVisibility(KoanTheme.pane(.automatic))
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
        // A television's queue heads itself, beside its controls.
        case .queue: Self.television ? "" : "Queue"
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

    #if os(tvOS)
    private static let television = true
    #else
    private static let television = false
    #endif
}

/// The Mac's toolbar filter, as the search field iOS puts under a page's title.
/// Its own modifier so that typing re-runs this and not the page.
private struct SectionFilter: ViewModifier {
    let placeholder: String?
    @Environment(LibraryModel.self) private var library

    func body(content: Content) -> some View {
        if let placeholder {
            @Bindable var library = library
            content.koanSearchable(text: $library.filter, prompt: placeholder)
        } else {
            content
        }
    }
}

/// The track browser's sort: the choices, ticked, under one control.
private struct TrackSortMenu: View {
    @Environment(LibraryModel.self) private var library

    var body: some View {
        SortMenu(
            selection: Binding(get: { library.trackSort }, set: { library.trackSort = $0 }),
            options: TrackBrowseSort.offered(recent: library.browseFilter.recent, searching: !library.filter.isEmpty)
                .map { ($0.label, $0) }
        )
    }
}

/// The album browser's sort, as `TrackSortMenu` is the track browser's.
private struct AlbumSortMenu: View {
    @Environment(LibraryModel.self) private var library

    var body: some View {
        SortMenu(
            selection: Binding(get: { library.albumSort }, set: { library.albumSort = $0 }),
            options: AlbumSort.offered(
                recent: library.browseFilter.recent, downloaded: library.browseFilter.downloaded,
                searching: !library.filter.isEmpty
            ).map { ($0.label, $0) }
        )
    }
}

/// A sort's choices under one control: a menu, or on a television in the
/// theme, a panel of the theme's rows.
private struct SortMenu<Value: Hashable>: View {
    @Binding var selection: Value
    let options: [(label: String, value: Value)]
    #if os(tvOS)
    @State private var open = false
    #endif

    var body: some View {
        #if os(tvOS)
        if KoanTheme.isOn {
            Button { open = true } label: {
                KoanLabel("Sort", icon: "arrow.up.arrow.down")
            }
            .televisionPanel(isPresented: $open, title: "Sort") {
                TelevisionChoices(
                    selection: $selection,
                    options: options.map { (KoanTheme.label($0.label), $0.value) }
                ) { open = false }
            }
        } else {
            menu
        }
        #else
        menu
        #endif
    }

    private var menu: some View {
        Menu {
            Picker("Sort", selection: $selection) {
                ForEach(options, id: \.value) { Text($0.label).tag($0.value) }
            }
        } label: {
            Label("Sort", systemImage: "arrow.up.arrow.down")
        }
    }
}

#if os(tvOS)
/// A listing's name filter, filters and sort on a television: a row above the
/// listing, reached by moving up from it, with room for their names. The
/// filter is a field rather than `.searchable`, whose keyboard would stand
/// across the top of the page over its title and these buttons.
private struct BrowseControlsRow: View {
    let section: Navigator.Section
    @Environment(LibraryModel.self) private var library

    var body: some View {
        @Bindable var library = library
        HStack(spacing: 24) {
            if let placeholder = section.filterPlaceholder {
                NameFilter(text: $library.filter, prompt: placeholder)
            }
            Spacer()
            if section.isBrowser {
                browserControls
            }
        }
        .padding(.horizontal, 80)
        .padding(.bottom, 16)
        .focusSection()
    }

    @ViewBuilder private var browserControls: some View {
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
}

/// Narrowing a listing by name on a television: the field opens the system's
/// keyboard, and the listing follows what is typed.
struct NameFilter: View {
    @Binding var text: String
    let prompt: String

    var body: some View {
        TextField(KoanTheme.label(prompt), text: $text)
            .autocorrectionDisabled()
            .textInputAutocapitalization(.never)
            .frame(width: 640)
            .koanField(text, prompt: KoanTheme.label(prompt))
            .accessibilityIdentifier("name-filter")
    }
}
#else
/// The track browser's sort, in the navigation bar.
private struct TrackSortControls: ToolbarContent {
    var body: some ToolbarContent {
        ToolbarItem(placement: .topBarTrailing) { TrackSortMenu().koanControl() }
            .sharedBackgroundVisibility(KoanTheme.pane(.automatic))
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
                .koanControl()
            }
            .sharedBackgroundVisibility(KoanTheme.pane(.automatic))
        }
        ToolbarItem(placement: .topBarTrailing) { AlbumSortMenu().koanControl() }
            .sharedBackgroundVisibility(KoanTheme.pane(.automatic))
    }
}
#endif
