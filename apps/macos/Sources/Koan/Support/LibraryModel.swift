import Foundation
import KoanFFI
import SwiftUI

/// How much history to hold. Enough to scroll back through an evening's
/// listening without paging; the whole table would be unbounded.
private let historyPageSize: UInt32 = 500

/// Library browsing state.
///
/// The full album and artist lists are loaded once, because search and the
/// detail views resolve ids against them. Narrowing them is the database's job:
/// filtering a few thousand rows in Swift cost sixteen milliseconds of main
/// thread per keystroke, which is a filter field that visibly lags the typing.
/// Tracks are never loaded wholesale; there are tens of thousands of them and
/// you only ever look at one album's worth at a time.
@MainActor
@Observable
final class LibraryModel {
    typealias Section = Navigator.Section

    let engine: KoanEngine

    /// What is on screen. Written only by the navigator, which owns it — the
    /// library follows where you are, it does not decide it.
    private(set) var section: Section = .queue

    /// The navigator moved. Catch up.
    func showing(_ section: Section) {
        guard section != self.section else { return }
        self.section = section
        // A filter you left behind on another view is invisible here, and an
        // apparently empty library is the result.
        filter = ""
        load()
    }

    /// Substring filter over whatever the current section is showing.
    var filter: String = "" {
        didSet {
            guard filter != oldValue else { return }
            refilter()
        }
    }

    /// In flight for the sections whose filter is a query. Long enough that a
    /// burst of typing is one round trip, short enough not to read as lag.
    private var filterQuery: Task<Void, Never>?
    private static let filterDebounce = Duration.milliseconds(120)

    /// Newest first by default: the record you just added is the one you're
    /// looking for. Persisted so it survives a relaunch.
    var albumSort: AlbumSort = .recentlyAdded {
        didSet {
            guard albumSort != oldValue else { return }
            UserDefaults.standard.set(albumSort.storageKey, forKey: "albumSort")
            reloadAlbums()
        }
    }

    private(set) var albums: [Album] = [] { didSet { refilter() } }
    private(set) var artists: [Artist] = [] { didSet { refilter() } }
    private(set) var favourites: [Track] = [] { didSet { refilter() } }

    // Favourite state is read from here rather than from the copy baked into
    // each Track when it was fetched. A track appears in the album view, the
    // artist view, the queue, the picker and search results, and refetching
    // every one of those after a heart click is neither cheap nor reliable —
    // it left the album view showing an unfilled heart on a track that was
    // already favourited.
    private(set) var favouriteTrackIds: Set<Int64> = []
    private(set) var favouriteAlbumIds: Set<Int64> = [] { didSet { refilter() } }
    private(set) var favouriteArtistIds: Set<Int64> = [] { didSet { refilter() } }

    func isFavourite(track id: Int64) -> Bool { favouriteTrackIds.contains(id) }
    func isFavourite(album id: Int64) -> Bool { favouriteAlbumIds.contains(id) }
    func isFavourite(artist id: Int64) -> Bool { favouriteArtistIds.contains(id) }
    private(set) var playHistory: [PlayHistoryEntry] = [] { didSet { refilter() } }
    private(set) var snapshots: [Snapshot] = []
    private(set) var stats: Stats?
    private(set) var isLoading = false

    private(set) var detailTracks: [Track] = []

    /// Set while a scan runs so the UI can show progress and refuse a second one.
    /// Set by `AppState`. Long tasks register here so one place can say what is
    /// happening — see `ActivityModel`.
    weak var activity: ActivityModel?

    private(set) var isScanning = false
    var scanSummary: ScanSummary?

    init(engine: KoanEngine) {
        self.engine = engine
        refreshFavourites()
        if let stored = UserDefaults.standard.string(forKey: "albumSort"),
           let sort = AlbumSort(storageKey: stored) {
            albumSort = sort
        }
    }

    /// Re-runs the current sort. Only visibly different under Random, which is
    /// reshuffled server-side on every call — that's what the button is for.
    func reshuffleAlbums() { reloadAlbums() }

    private func reloadAlbums() {
        let engine = self.engine
        let sort = albumSort
        Task {
            albums = (try? await engine.albums(artistId: nil, sort: sort, search: nil)) ?? []
            reindex()
        }
    }

    // MARK: - Filtered views

    /// Stored rather than computed. A `List` reads its collection far more than
    /// once per update, and narrowing it on every read froze the artist list for
    /// a second or two whenever the filter changed. The album grid is lazy and
    /// never noticed, which is what made it look like a bug in the artist view
    /// specifically.
    private(set) var visibleAlbums: [Album] = []
    private(set) var visibleArtists: [Artist] = []
    private(set) var visibleFavourites: [Track] = []
    /// Favourited records and artists, resolved out of the catalogue already in
    /// memory — the engine hands back ids, and `prefetchCatalogue` holds the
    /// rows. Taken in the catalogue's order rather than the set's, which has
    /// none, so the grid does not reshuffle itself on every toggle.
    private(set) var visibleFavouriteAlbums: [Album] = []
    private(set) var visibleFavouriteArtists: [Artist] = []
    private(set) var visiblePlayHistory: [PlayHistoryEntry] = []

    /// Recompute what each section shows. Called whenever the filter or any of
    /// the underlying collections change.
    ///
    /// Switching section clears the filter, so only the section on screen can
    /// hold one and every other collection is handed over whole. Albums and
    /// artists are unbounded and go to the database; favourites and a page of
    /// history are small enough to narrow here.
    private func refilter() {
        visibleFavourites = section == .favourites
            ? matching(favourites) { [$0.title, $0.artistName, $0.albumTitle] }
            : favourites
        // Only ever built for the page that shows them: this walks the whole
        // catalogue, and every other section would be paying for it on each
        // keystroke of its own filter.
        if section == .favourites {
            visibleFavouriteAlbums = matching(
                albums.filter { favouriteAlbumIds.contains($0.id) }
            ) { [$0.title, $0.artistName] }
            visibleFavouriteArtists = matching(
                artists.filter { favouriteArtistIds.contains($0.id) }
            ) { [$0.name] }
        } else {
            visibleFavouriteAlbums = []
            visibleFavouriteArtists = []
        }
        visiblePlayHistory = section == .playHistory
            ? matching(playHistory) { [$0.track.title, $0.track.artistName, $0.track.albumTitle] }
            : playHistory

        guard section == .albums || section == .artists, !filter.isEmpty else {
            filterQuery?.cancel()
            visibleAlbums = albums
            visibleArtists = artists
            return
        }
        runFilterQuery()
    }

    /// Ask the database for the matches.
    ///
    /// Debounced and cancellable, so holding a key down is one query rather than
    /// one per character and an answer to a filter you have already typed past
    /// never lands.
    private func runFilterQuery() {
        filterQuery?.cancel()
        let engine = self.engine
        let wantsAlbums = section == .albums
        let sort = albumSort
        let query = filter
        filterQuery = Task {
            try? await Task.sleep(for: Self.filterDebounce)
            guard !Task.isCancelled else { return }
            if wantsAlbums {
                let rows = try? await engine.albums(
                    artistId: nil, sort: sort, search: query
                )
                guard !Task.isCancelled else { return }
                visibleAlbums = rows ?? []
            } else {
                let rows = try? await engine.artists(search: query)
                guard !Task.isCancelled else { return }
                visibleArtists = rows ?? []
            }
        }
    }

    private func matching<T>(_ rows: [T], _ fields: (T) -> [String]) -> [T] {
        guard !filter.isEmpty else { return rows }
        return rows.filter { row in
            fields(row).contains { $0.localizedCaseInsensitiveContains(filter) }
        }
    }

    // MARK: - Loading

    func loadInitial() {
        loadStats()
        load()
        // Search resolves fuzzy match ids against these, so they cannot wait
        // until their section is first visited.
        prefetchCatalogue()
    }

    private var albumsById: [Int64: Album] = [:]
    private var artistsById: [Int64: Artist] = [:]

    func album(id: Int64) -> Album? { albumsById[id] }
    func artist(id: Int64) -> Artist? { artistsById[id] }

    private func prefetchCatalogue() {
        let engine = self.engine
        let sort = albumSort
        Task {
            if albums.isEmpty {
                albums = (try? await engine.albums(artistId: nil, sort: sort, search: nil)) ?? []
            }
            if artists.isEmpty {
                artists = (try? await engine.artists(search: nil)) ?? []
            }
            reindex()
        }
    }

    private func reindex() {
        albumsById = Dictionary(albums.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
        artistsById = Dictionary(artists.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
    }

    /// Loads whatever the current section needs. Everything heavy happens off
    /// the main actor; only the assignment comes back.
    func load() {
        let engine = self.engine
        let section = self.section
        let sort = albumSort
        isLoading = true

        Task {
            switch section {
            case .queue, .searchResults:
                break  // owned by the player and search models respectively
            case .albums:
                if albums.isEmpty {
                    albums = (try? await engine.albums(artistId: nil, sort: sort, search: nil)) ?? []
                    reindex()
                }
            case .artists:
                if artists.isEmpty {
                    artists = (try? await engine.artists(search: nil)) ?? []
                    reindex()
                }
            case .favourites:
                favourites = (try? await engine.favourites()) ?? []
            case .playHistory:
                // Always refetched: it changes underneath you as you listen.
                playHistory = (try? await engine.playHistory(limit: historyPageSize, offset: 0)) ?? []
            case .snapshots:
                snapshots = (try? await engine.snapshots()) ?? []
            }
            isLoading = false
        }
    }

    /// Forget specific plays. The tracks are untouched; only the log changes.
    func forgetPlays(ids: Set<Int64>) {
        guard !ids.isEmpty else { return }
        let engine = self.engine
        let doomed = Array(ids)
        // Dropped locally first so the list does not visibly lag the keystroke.
        playHistory.removeAll { ids.contains($0.id) }
        Task { _ = try? await engine.deletePlays(ids: doomed) }
    }

    /// Forget every play.
    func clearPlayHistory() {
        let engine = self.engine
        Task {
            _ = try? await engine.clearPlayHistory()
            playHistory = []
        }
    }

    func loadStats() {
        let engine = self.engine
        Task {
            stats = try? await engine.libraryStats()
        }
    }

    /// Tracks for the album detail pane.
    func loadTracks(albumId: Int64) {
        let engine = self.engine
        detailTracks = []
        Task {
            detailTracks = (try? await engine.tracks(
                albumId: albumId, artistId: nil, sort: .album, limit: 500, offset: 0
            )) ?? []
        }
    }

    // MARK: - Mutations

    /// Toggle a track favourite and reflect it everywhere at once.
    ///
    /// The engine returns the new state, so the id sets are updated from that
    /// rather than by re-reading the database — the row responds on the click
    /// rather than a round trip later.
    func toggleFavourite(track id: Int64) {
        let engine = self.engine
        Task {
            let now = (try? await engine.toggleFavourite(trackId: id))
            guard let now else { return }
            if now { favouriteTrackIds.insert(id) } else { favouriteTrackIds.remove(id) }
            reloadFavouritesList()
        }
    }

    func toggleFavourite(album id: Int64) {
        let engine = self.engine
        Task {
            let now = (try? await engine.toggleFavouriteAlbum(albumId: id))
            guard let now else { return }
            if now { favouriteAlbumIds.insert(id) } else { favouriteAlbumIds.remove(id) }
        }
    }

    func toggleFavourite(artist id: Int64) {
        let engine = self.engine
        Task {
            let now = (try? await engine.toggleFavouriteArtist(artistId: id))
            guard let now else { return }
            if now { favouriteArtistIds.insert(id) } else { favouriteArtistIds.remove(id) }
        }
    }

    /// Re-read every favourite id from the database. Called after a sync, which
    /// can change them without going through a toggle.
    func refreshFavourites() {
        let engine = self.engine
        Task {
            let sets = (
                Set((try? await engine.favouriteTrackIds()) ?? []),
                Set((try? await engine.favouriteAlbumIds()) ?? []),
                Set((try? await engine.favouriteArtistIds()) ?? [])
            )
            favouriteTrackIds = sets.0
            favouriteAlbumIds = sets.1
            favouriteArtistIds = sets.2
            reloadFavouritesList()
        }
    }

    private func reloadFavouritesList() {
        guard section == .favourites else { return }
        let engine = self.engine
        Task {
            favourites = (try? await engine.favourites()) ?? []
        }
    }

    func saveSnapshot(name: String) {
        let engine = self.engine
        Task {
            try? await engine.saveSnapshot(name: name)
            load()
        }
    }

    func deleteSnapshot(name: String) {
        let engine = self.engine
        Task {
            _ = try? await engine.deleteSnapshot(name: name)
            load()
        }
    }

    /// Pull the remote library. Minutes on a large server, so it runs detached
    /// and the caches are dropped afterwards rather than during.
    func syncRemote(full: Bool = false) {
        guard !isScanning else { return }
        isScanning = true
        let engine = self.engine
        let job = activity?.begin(
            full ? "Full sync with server" : "Syncing with server",
            exclusive: true
        )
        Task {
            _ = try? await engine.syncRemote(full: full)
            if let job { activity?.end(job) }
            isScanning = false
            albums = []
            artists = []
            loadStats()
            prefetchCatalogue()
            load()
        }
    }

    /// Full rescan of every configured folder. Minutes on a big library, so it
    /// runs detached and the UI stays live throughout.
    func scan(force: Bool = false) {
        guard !isScanning else { return }
        isScanning = true
        scanSummary = nil

        let engine = self.engine
        let job = activity?.begin(
            force ? "Rescanning every file" : "Scanning library",
            exclusive: true,
            cancellable: true
        )
        let progress = job.flatMap { activity?.reporter(for: $0) }
        Task {
            let result = try? await engine.scanReporting(force: force, reporter: progress)
            if let job { activity?.end(job) }
            scanSummary = result
            isScanning = false
            libraryChanged()
        }
    }

    /// Rows appeared or vanished underneath us. Albums and artists are loaded
    /// once and filtered in memory, so they have to be dropped rather than
    /// merged — anything else leaves the browser showing a library that no
    /// longer exists.
    func libraryChanged() {
        albums = []
        artists = []
        loadStats()
        load()
    }
}
