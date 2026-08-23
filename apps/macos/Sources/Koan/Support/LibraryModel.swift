import Foundation
import KoanFFI
import SwiftUI

/// Library browsing state.
///
/// Albums and artists are loaded once and filtered in memory — a few thousand
/// rows is nothing, and it makes the filter field feel instant. Tracks are
/// never loaded wholesale; there are tens of thousands of them and you only
/// ever look at one album's worth at a time.
@MainActor
@Observable
final class LibraryModel {
    enum Section: Hashable {
        case queue
        case searchResults
        case albums
        case artists
        case favourites
        case snapshots
    }

    let engine: KoanEngine

    var section: Section = .queue {
        didSet {
            guard section != oldValue else { return }
            // Each section is its own stack conceptually; carrying a path
            // across a switch would strand you on an unrelated detail view.
            path = NavigationPath()
            // A filter you left behind on another view is invisible here, and
            // an apparently empty library is the result.
            filter = ""
            load()
            if !navigatingHistory { record(.section(section)) }
        }
    }

    /// Substring filter over whatever the current section is showing.
    var filter: String = "" {
        didSet {
            guard filter != oldValue else { return }
            refilter()
        }
    }

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
    private(set) var favouriteAlbumIds: Set<Int64> = []
    private(set) var favouriteArtistIds: Set<Int64> = []

    func isFavourite(track id: Int64) -> Bool { favouriteTrackIds.contains(id) }
    func isFavourite(album id: Int64) -> Bool { favouriteAlbumIds.contains(id) }
    func isFavourite(artist id: Int64) -> Bool { favouriteArtistIds.contains(id) }
    private(set) var snapshots: [Snapshot] = []
    private(set) var stats: Stats?
    private(set) var isLoading = false

    /// The browse stack's path. Lives here so search can push a destination
    /// rather than each browser owning a stack nothing else can reach.
    var path = NavigationPath()

    /// Back to the top of the current section, without leaving it.
    func popToRoot() {
        guard !path.isEmpty else { return }
        path = NavigationPath()
        selectedAlbumId = nil
        selectedArtistId = nil
    }

    var selectedArtistId: Int64? {
        didSet {
            guard selectedArtistId != oldValue else { return }
            refilter()
        }
    }
    var selectedAlbumId: Int64?
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
            albums = await Task.detached(priority: .userInitiated) {
                (try? engine.albums(artistId: nil, sort: sort)) ?? []
            }.value
            reindex()
        }
    }

    // MARK: - Filtered views

    /// Stored rather than computed. A `List` reads its collection far more than
    /// once per update, and re-filtering forty thousand rows on every read
    /// froze the artist list for a second or two whenever the filter changed.
    /// The album grid is lazy and never noticed, which is what made it look
    /// like a bug in the artist view specifically.
    private(set) var visibleAlbums: [Album] = []
    private(set) var visibleArtists: [Artist] = []
    private(set) var visibleFavourites: [Track] = []

    /// Recompute what each section shows. Called whenever the filter or any of
    /// the underlying collections change.
    private func refilter() {
        let scoped = selectedArtistId.map { id in albums.filter { $0.artistId == id } } ?? albums
        guard !filter.isEmpty else {
            visibleAlbums = scoped
            visibleArtists = artists
            visibleFavourites = favourites
            return
        }
        visibleAlbums = scoped.filter {
            $0.title.localizedCaseInsensitiveContains(filter)
                || $0.artistName.localizedCaseInsensitiveContains(filter)
        }
        visibleArtists = artists.filter {
            $0.name.localizedCaseInsensitiveContains(filter)
        }
        visibleFavourites = favourites.filter {
            $0.title.localizedCaseInsensitiveContains(filter)
                || $0.artistName.localizedCaseInsensitiveContains(filter)
                || $0.albumTitle.localizedCaseInsensitiveContains(filter)
        }
    }

    var selectedAlbum: Album? {
        selectedAlbumId.flatMap { id in albums.first { $0.id == id } }
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
                albums = await Task.detached(priority: .utility) {
                    (try? engine.albums(artistId: nil, sort: sort)) ?? []
                }.value
            }
            if artists.isEmpty {
                artists = await Task.detached(priority: .utility) {
                    (try? engine.artists(search: nil)) ?? []
                }.value
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
                    albums = await Task.detached(priority: .userInitiated) {
                        (try? engine.albums(artistId: nil, sort: sort)) ?? []
                    }.value
                    reindex()
                }
            case .artists:
                if artists.isEmpty {
                    artists = await Task.detached(priority: .userInitiated) {
                        (try? engine.artists(search: nil)) ?? []
                    }.value
                    reindex()
                }
            case .favourites:
                favourites = await Task.detached(priority: .userInitiated) {
                    (try? engine.favourites()) ?? []
                }.value
            case .snapshots:
                snapshots = await Task.detached(priority: .userInitiated) {
                    (try? engine.snapshots()) ?? []
                }.value
            }
            isLoading = false
        }
    }

    func loadStats() {
        let engine = self.engine
        Task {
            stats = await Task.detached(priority: .utility) { try? engine.libraryStats() }.value
        }
    }

    /// Tracks for the album detail pane.
    func loadTracks(albumId: Int64) {
        let engine = self.engine
        detailTracks = []
        Task {
            detailTracks = await Task.detached(priority: .userInitiated) {
                (try? engine.tracks(
                    albumId: albumId, artistId: nil, sort: .album, limit: 500, offset: 0
                )) ?? []
            }.value
        }
    }

    /// Show the queue. Called after anything that starts playback outright, so
    /// you end up looking at what you just started. Not called for "add to
    /// queue" or "play next" — those are things you do while browsing, and
    /// being thrown across the app for them would be rude.
    func showQueue() {
        section = .queue
    }

    /// Show the queue once it holds what was just started.
    ///
    /// Queue mutations run off the main actor, so switching immediately shows
    /// the old queue for a frame or two and then flickers. Waiting for the
    /// engine to confirm avoids that — but only briefly: if the mutation is
    /// slow enough to notice, jumping to the queue after the fact would feel
    /// like the app moving on its own, so it stays put instead.
    func showQueueWhenReady(watching player: PlayerModel) {
        let before = player.queueVersion
        Task {
            let deadline = ContinuousClock.now + .milliseconds(50)
            while ContinuousClock.now < deadline {
                if player.queueVersion != before {
                    section = .queue
                    return
                }
                try? await Task.sleep(for: .milliseconds(5))
            }
        }
    }

    /// Jump straight to a thing from search: switch to the section it lives in,
    /// then push its detail view.
    /// The track that search sent you here for, so the album view can single it
    /// out. Cleared once the view has scrolled to it.
    var highlightedTrackId: Int64?

    // MARK: - History
    //
    // A NavigationStack only goes back within one stack, so it can't return you
    // across a section switch or a jump from search. This records every
    // destination the user actually reached, which is what "back" means to
    // someone using the app.

    enum Destination: Hashable {
        case section(Section)
        case album(Int64)
        case artist(Int64)
    }

    /// What the sidebar should highlight.
    ///
    /// Reaching an album from Favourites, from search, or from "Go to Album" in
    /// the queue pushes a detail view without changing section, so the sidebar
    /// went on pointing at wherever you started — which is not where you are.
    /// The row an album lives under is Albums, whichever door you came through.
    var navSelection: Section {
        guard !path.isEmpty else { return section }
        switch history.indices.contains(historyCursor) ? history[historyCursor] : .section(section) {
        case .album: return .albums
        case .artist: return .artists
        case .section(let s): return s
        }
    }

    private(set) var history: [Destination] = [.section(.queue)]
    private(set) var historyCursor = 0
    /// Set while replaying history, so applying a destination doesn't record it
    /// again and trap the user in a loop.
    private var navigatingHistory = false

    var canGoBack: Bool { historyCursor > 0 }
    var canGoForward: Bool { historyCursor < history.count - 1 }

    private func record(_ destination: Destination) {
        // A new move discards anything ahead, the way a browser does.
        if historyCursor < history.count - 1 {
            history.removeSubrange((historyCursor + 1)...)
        }
        guard history.last != destination else { return }
        history.append(destination)
        historyCursor = history.count - 1
    }

    func goBack() {
        guard canGoBack else { return }
        historyCursor -= 1
        apply(history[historyCursor])
    }

    func goForward() {
        guard canGoForward else { return }
        historyCursor += 1
        apply(history[historyCursor])
    }

    private func apply(_ destination: Destination) {
        navigatingHistory = true
        defer { navigatingHistory = false }
        switch destination {
        case .section(let target):
            section = target
            path = NavigationPath()
        case .album(let id):
            path = NavigationPath()
            path.append(AlbumRoute(id: id))
        case .artist(let id):
            path = NavigationPath()
            path.append(ArtistRoute(id: id))
        }
    }

    /// Push a destination onto the current stack.
    ///
    /// Deliberately does *not* switch `section` first. Doing so swaps the
    /// stack's root view in the same update, and SwiftUI discards the path
    /// against the old root — which landed you on the plain album list instead
    /// of the album. Pushing onto whatever stack is showing also gives you a
    /// Back button to the results you came from.
    func reveal(album id: Int64, highlighting trackId: Int64? = nil) {
        highlightedTrackId = trackId
        path.append(AlbumRoute(id: id))
        if !navigatingHistory { record(.album(id)) }
    }

    func reveal(artist id: Int64) {
        path.append(ArtistRoute(id: id))
        if !navigatingHistory { record(.artist(id)) }
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
            let now = await Task.detached(priority: .userInitiated) {
                (try? engine.toggleFavourite(trackId: id))
            }.value
            guard let now else { return }
            if now { favouriteTrackIds.insert(id) } else { favouriteTrackIds.remove(id) }
            reloadFavouritesList()
        }
    }

    func toggleFavourite(album id: Int64) {
        let engine = self.engine
        Task {
            let now = await Task.detached(priority: .userInitiated) {
                (try? engine.toggleFavouriteAlbum(albumId: id))
            }.value
            guard let now else { return }
            if now { favouriteAlbumIds.insert(id) } else { favouriteAlbumIds.remove(id) }
        }
    }

    func toggleFavourite(artist id: Int64) {
        let engine = self.engine
        Task {
            let now = await Task.detached(priority: .userInitiated) {
                (try? engine.toggleFavouriteArtist(artistId: id))
            }.value
            guard let now else { return }
            if now { favouriteArtistIds.insert(id) } else { favouriteArtistIds.remove(id) }
        }
    }

    /// Re-read every favourite id from the database. Called after a sync, which
    /// can change them without going through a toggle.
    func refreshFavourites() {
        let engine = self.engine
        Task {
            let sets = await Task.detached(priority: .utility) {
                (
                    Set((try? engine.favouriteTrackIds()) ?? []),
                    Set((try? engine.favouriteAlbumIds()) ?? []),
                    Set((try? engine.favouriteArtistIds()) ?? [])
                )
            }.value
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
            favourites = await Task.detached(priority: .utility) {
                (try? engine.favourites()) ?? []
            }.value
        }
    }

    func saveSnapshot(name: String) {
        let engine = self.engine
        Task {
            await Task.detached(priority: .utility) { try? engine.saveSnapshot(name: name) }.value
            load()
        }
    }

    func deleteSnapshot(name: String) {
        let engine = self.engine
        Task {
            _ = await Task.detached(priority: .utility) { try? engine.deleteSnapshot(name: name) }.value
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
            _ = await Task.detached(priority: .utility) {
                try? engine.syncRemote(full: full)
            }.value
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
            let result = await Task.detached(priority: .utility) {
                try? engine.scanReporting(force: force, reporter: progress)
            }.value
            if let job { activity?.end(job) }
            scanSummary = result
            isScanning = false
            // The library moved underneath us — drop the caches and reload.
            albums = []
            artists = []
            loadStats()
            load()
        }
    }
}
