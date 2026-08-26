import Foundation
import KoanFFI
import SwiftUI

/// How much of a section to hold at once.
///
/// Enough that the first screenful is never short and scrolling always has
/// somewhere to go; small enough that opening the browser costs the same on a
/// library of five thousand records as on one of fifty.
private let pageSize: UInt32 = 200

/// Library browsing state.
///
/// Nothing here is a copy of the library. Each section holds the page it is
/// showing and asks for the next one when the scroll reaches the end;
/// narrowing, sorting and paging all happen in SQL. koan-core owns the library,
/// so the only questions this can answer without asking are the ones about
/// where the user is.
///
/// The consequence worth knowing: there is no load to have forgotten to do. A
/// section that has never been visited shows the library the first time it is,
/// and one whose rows changed underneath it asks again rather than merging.
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
        // apparently empty library is the result. Clearing it reloads by
        // itself; if there was nothing to clear, ask outright.
        let hadFilter = !filter.isEmpty
        filter = ""
        if !hadFilter { reload() }
    }

    /// Substring filter over whatever the current section is showing. It
    /// narrows the query, not the answer.
    var filter: String = "" {
        didSet {
            guard filter != oldValue else { return }
            reload(debounced: true)
        }
    }

    /// Long enough that a burst of typing is one round trip, short enough not
    /// to read as lag.
    private static let filterDebounce = Duration.milliseconds(120)

    /// Newest first by default: the record you just added is the one you're
    /// looking for. Persisted so it survives a relaunch.
    var albumSort: AlbumSort = .recentlyAdded {
        didSet {
            guard albumSort != oldValue else { return }
            UserDefaults.standard.set(albumSort.storageKey, forKey: "albumSort")
            reload()
        }
    }

    /// Which shuffle Random means right now. Fixed for the whole listing, so
    /// page two belongs to the same shuffle as page one rather than reshuffling
    /// underneath the scroll.
    private var shuffleSeed = Int64.random(in: .min ... .max)

    /// Deal again. Only visibly different under Random, which is what the
    /// button is for.
    func reshuffleAlbums() {
        shuffleSeed = Int64.random(in: .min ... .max)
        reload()
    }

    // MARK: - What each section is showing

    /// A page of rows, not a filtered copy of a catalogue. Stored rather than
    /// computed because a `List` reads its collection far more than once per
    /// update, and anything derived on read is derived a few hundred times a
    /// frame.
    private(set) var visibleAlbums: [Album] = []
    private(set) var visibleArtists: [Artist] = []
    private(set) var visibleFavourites: [Track] = []
    private(set) var visibleFavouriteAlbums: [Album] = []
    private(set) var visibleFavouriteArtists: [Artist] = []
    private(set) var visiblePlayHistory: [PlayHistoryEntry] = []

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

    private(set) var stats: Stats?
    private(set) var isLoading = false

    private(set) var detailTracks: [Track] = []

    /// Set while a scan runs so the UI can show progress and refuse a second
    /// one. Set by `AppState`. Long tasks register here so one place can say
    /// what is happening — see `ActivityModel`.
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

    // MARK: - Loading

    func loadInitial() {
        loadStats()
        reload()
    }

    private var loading: Task<Void, Never>?
    private var paging: Task<Void, Never>?
    /// Set once a page comes back short: there is no more to ask for, and
    /// scrolling should stop trying.
    private var exhausted = false
    private var isPaging = false

    /// Ask for the first page of whatever is on screen.
    ///
    /// Cancellable, so an answer to a filter you have already typed past never
    /// lands, and debounced when a keystroke caused it, so holding a key down
    /// is one query rather than one per character.
    func reload(debounced: Bool = false) {
        loading?.cancel()
        paging?.cancel()
        exhausted = false
        isPaging = false
        isLoading = true

        let request = self.request
        loading = Task {
            if debounced {
                try? await Task.sleep(for: Self.filterDebounce)
                guard !Task.isCancelled else { return }
            }
            let rows = await request.page(offset: 0)
            guard !Task.isCancelled else { return }
            show(rows, appending: false)
            isLoading = false
        }
    }

    /// The scroll reached the end of what we hold. Called by the last row on
    /// screen, which is the only thing that knows.
    func loadMore() {
        guard !exhausted, !isPaging, !isLoading else { return }
        let request = self.request
        let offset = loadedCount
        guard offset > 0 else { return }
        isPaging = true

        paging = Task {
            let rows = await request.page(offset: UInt32(offset))
            guard !Task.isCancelled else { return }
            show(rows, appending: true)
            isPaging = false
        }
    }

    /// Everything a section's query depends on, captured off the model so the
    /// answer that lands belongs to the question that was asked. Anything that
    /// changes one cancels the task holding it.
    private struct Request {
        let section: Section
        let filter: String
        let sort: AlbumSort
        let seed: Int64
        let engine: KoanEngine

        var search: String? { filter.isEmpty ? nil : filter }

        /// One page of this section, or everything it has if it does not page.
        func page(offset: UInt32) async -> Rows {
            switch section {
            case .queue, .searchResults, .playlist:
                // Owned by the player, search and playlist models respectively.
                return .none
            case .albums:
                return .albums(
                    (try? await engine.albums(
                        artistId: nil, sort: sort, seed: seed, search: search,
                        limit: pageSize, offset: offset
                    )) ?? []
                )
            case .artists:
                return .artists(
                    (try? await engine.artists(search: search, limit: pageSize, offset: offset))
                        ?? []
                )
            case .favourites:
                // Unpaged: a favourites list is bounded by what someone
                // troubled themselves to press a heart on.
                guard offset == 0 else { return .none }
                async let tracks = engine.favourites(search: search)
                async let albums = engine.favouriteAlbums(search: search)
                async let artists = engine.favouriteArtists(search: search)
                return .favourites(
                    tracks: (try? await tracks) ?? [],
                    albums: (try? await albums) ?? [],
                    artists: (try? await artists) ?? []
                )
            case .playHistory:
                return .history(
                    (try? await engine.playHistory(
                        search: search, limit: pageSize, offset: offset
                    )) ?? []
                )
            }
        }
    }

    /// How many rows the section on screen holds, which is where the next page
    /// starts. Sections that do not page never ask.
    private var loadedCount: Int {
        switch section {
        case .albums: visibleAlbums.count
        case .artists: visibleArtists.count
        case .playHistory: visiblePlayHistory.count
        default: 0
        }
    }

    private enum Rows {
        case none
        case albums([Album])
        case artists([Artist])
        case favourites(tracks: [Track], albums: [Album], artists: [Artist])
        case history([PlayHistoryEntry])
    }

    private var request: Request {
        Request(
            section: section, filter: filter, sort: albumSort, seed: shuffleSeed, engine: engine
        )
    }

    private func show(_ rows: Rows, appending: Bool) {
        switch rows {
        case .none:
            exhausted = true
        case .albums(let rows):
            visibleAlbums = appending ? visibleAlbums + rows : rows
            exhausted = rows.count < pageSize
        case .artists(let rows):
            visibleArtists = appending ? visibleArtists + rows : rows
            exhausted = rows.count < pageSize
        case .favourites(let tracks, let albums, let artists):
            visibleFavourites = tracks
            visibleFavouriteAlbums = albums
            visibleFavouriteArtists = artists
            exhausted = true
        case .history(let rows):
            visiblePlayHistory = appending ? visiblePlayHistory + rows : rows
            exhausted = rows.count < pageSize
        }
    }

    /// Forget specific plays. The tracks are untouched; only the log changes.
    func forgetPlays(ids: Set<Int64>) {
        guard !ids.isEmpty else { return }
        let engine = self.engine
        let doomed = Array(ids)
        // Dropped locally first so the list does not visibly lag the keystroke.
        visiblePlayHistory.removeAll { ids.contains($0.id) }
        Task { _ = try? await engine.deletePlays(ids: doomed) }
    }

    /// Forget every play.
    func clearPlayHistory() {
        let engine = self.engine
        Task {
            _ = try? await engine.clearPlayHistory()
            visiblePlayHistory = []
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
            reloadFavourites()
        }
    }

    func toggleFavourite(album id: Int64) {
        let engine = self.engine
        Task {
            let now = (try? await engine.toggleFavouriteAlbum(albumId: id))
            guard let now else { return }
            if now { favouriteAlbumIds.insert(id) } else { favouriteAlbumIds.remove(id) }
            reloadFavourites()
        }
    }

    func toggleFavourite(artist id: Int64) {
        let engine = self.engine
        Task {
            let now = (try? await engine.toggleFavouriteArtist(artistId: id))
            guard let now else { return }
            if now { favouriteArtistIds.insert(id) } else { favouriteArtistIds.remove(id) }
            reloadFavourites()
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
            reloadFavourites()
        }
    }

    /// The favourites page lists what the hearts say, so a toggle changes it.
    private func reloadFavourites() {
        guard section == .favourites else { return }
        reload()
    }

    /// Pull the remote library. Minutes on a large server, so it runs detached
    /// and the page is asked for again afterwards rather than during.
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
            libraryChanged()
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

    /// Rows appeared or vanished underneath us. Nothing to merge or invalidate:
    /// ask again.
    func libraryChanged() {
        loadStats()
        reload()
    }
}
