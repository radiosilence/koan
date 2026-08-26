import Foundation
import KoanFFI
import SwiftUI

/// Library browsing state.
///
/// Nothing here is derived, indexed or narrowed. A section asks koan-core what
/// it should be showing and shows exactly that; narrowing and sorting happen in
/// SQL, because the database is the only thing that knows the answer and asking
/// it is cheaper than keeping one.
///
/// Nothing is paged either. This is an in-process call, not a wire: a listing
/// arrives whole, so the scrollbar tells the truth about how long the library
/// is and one flick reaches the end of it.
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

    /// Which shuffle Random means right now. Held rather than dealt afresh on
    /// every read, so typing in the filter narrows the shuffle you are looking
    /// at instead of dealing a new one on each keystroke.
    private var shuffleSeed = Int64.random(in: .min ... .max)

    /// Deal again. Only visibly different under Random, which is what the
    /// button is for.
    func reshuffleAlbums() {
        shuffleSeed = Int64.random(in: .min ... .max)
        reload()
    }

    // MARK: - What each section is showing

    /// What the section on screen is showing, as the database handed it over.
    /// Stored rather than computed because a `List` reads its collection far
    /// more than once per update, and anything derived on read is derived a few
    /// hundred times a frame.
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

    /// Ask for whatever is on screen.
    ///
    /// Cancellable, so an answer to a filter you have already typed past never
    /// lands, and debounced when a keystroke caused it, so holding a key down
    /// is one query rather than one per character.
    func reload(debounced: Bool = false) {
        isLoading = true
        loading?.cancel()

        let request = self.request
        loading = Task {
            if debounced {
                try? await Task.sleep(for: Self.filterDebounce)
                guard !Task.isCancelled else { return }
            }
            let rows = await request.rows()
            guard !Task.isCancelled else { return }
            show(rows)
            isLoading = false
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

        /// Everything this section is showing.
        func rows() async -> Rows {
            switch section {
            case .queue, .searchResults, .playlist:
                // Owned by the player, search and playlist models respectively.
                return .none
            case .albums:
                return .albums(
                    (try? await engine.albums(
                        artistId: nil, sort: sort, seed: seed, search: search
                    )) ?? []
                )
            case .artists:
                return .artists((try? await engine.artists(search: search)) ?? [])
            case .favourites:
                // Three questions, asked at once — they are answers to the same
                // one and the page shows them together.
                async let tracks = engine.favourites(search: search)
                async let albums = engine.favouriteAlbums(search: search)
                async let artists = engine.favouriteArtists(search: search)
                return .favourites(
                    tracks: (try? await tracks) ?? [],
                    albums: (try? await albums) ?? [],
                    artists: (try? await artists) ?? []
                )
            case .playHistory:
                return .history((try? await engine.playHistory(search: search)) ?? [])
            }
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

    private func show(_ rows: Rows) {
        switch rows {
        case .none:
            break
        case .albums(let rows):
            visibleAlbums = rows
        case .artists(let rows):
            visibleArtists = rows
        case .favourites(let tracks, let albums, let artists):
            visibleFavourites = tracks
            visibleFavouriteAlbums = albums
            visibleFavouriteArtists = artists
        case .history(let rows):
            visiblePlayHistory = rows
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

    /// Pull the remote library. Minutes on a large server, so it runs detached.
    /// Nothing here refreshes anything: the engine announces the rows it wrote,
    /// and `libraryChanged()` runs off that.
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
        }
    }

    /// Throw away every file cached from the server.
    ///
    /// The library rows stay — they are what the server said exists — so the
    /// tracks remain playable and simply download again on demand. Exclusive
    /// like the other library tasks: it clears the cached paths off every row.
    func clearDownloads() {
        guard !isScanning else { return }
        isScanning = true
        let engine = self.engine
        let job = activity?.begin("Clearing downloaded files", exclusive: true)
        Task {
            _ = try? await engine.clearDownloadCache()
            if let job { activity?.end(job) }
            isScanning = false
            loadStats()
        }
    }

    /// Throw away the downloaded copies of these tracks.
    ///
    /// Not exclusive like the library-wide tasks: it touches only the rows
    /// named, and someone clearing one record should not have to wait behind a
    /// scan. Anything playing from a copy being removed keeps playing — the
    /// decoder has the file open, and unlinking it only takes the name away.
    func clearDownloads(trackIds: [Int64]) {
        guard !trackIds.isEmpty else { return }
        let engine = self.engine
        Task {
            _ = try? await engine.clearDownloads(trackIds: trackIds)
            loadStats()
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
        }
    }

    /// Rows appeared or vanished underneath us — a scan, a sync, an import, or
    /// a folder being forgotten. Whether this app asked for it or the engine
    /// did it on its own makes no difference here: nothing to merge, nothing to
    /// invalidate, just ask again.
    ///
    /// Favourites too, because a sync reconciles them with the server and the
    /// hearts on screen are stale the moment it lands.
    func libraryChanged() {
        loadStats()
        refreshFavourites()
        reload()
    }
}
