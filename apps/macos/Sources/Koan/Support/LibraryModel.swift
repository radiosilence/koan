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

    /// A section and everything it shows, as one value.
    ///
    /// Read before the navigator moves — see `prepare(section:)`. A section
    /// that arrives first and asks afterwards draws itself empty, and the empty
    /// state of a listing is the word "No albums yet" over a page that has
    /// albums.
    struct Listing {
        let section: Section
        fileprivate let rows: Rows
    }

    /// What a section will be showing, without touching what is on screen.
    ///
    /// `nil` when it is already showing: the rows are in hand and the filter
    /// over them is somebody's, so a move back onto a section is not a reason
    /// to re-read it or to throw their narrowing away. A library change reloads
    /// it where it is drawn instead.
    func prepare(section: Section) async -> Listing? {
        guard section != self.section else { return nil }
        // Nothing carries over: a filter you left behind on another view is
        // invisible here, and an apparently empty library is the result.
        return await Listing(
            section: section,
            rows: Request(
                section: section, filter: "", sort: albumSort, seed: shuffleSeed, engine: engine
            ).detached()
        )
    }

    /// Adopt a listing, at the moment the navigator moves to it.
    func show(_ listing: Listing) {
        loading?.cancel()
        section = listing.section
        // Quietly: the rows for this section are already in hand, so emptying
        // the filter it arrives with is not a reason to ask for them again.
        adopting = true
        filter = ""
        adopting = false
        show(listing.rows)
        isLoading = false
    }

    /// Substring filter over whatever the current section is showing. It
    /// narrows the query, not the answer.
    var filter: String = "" {
        didSet {
            guard filter != oldValue, !adopting else { return }
            reload(debounced: true)
        }
    }

    /// True while a prepared listing is being adopted — see `show(_:)`.
    private var adopting = false

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

    /// Where long tasks register, so one place can say what is happening and
    /// refuse a second task that would collide with a running one. Set by
    /// `AppState` — see `ActivityModel`.
    weak var activity: ActivityModel?
    /// Set by `AppState`, so a record's sleeve can be warmed as its rows are
    /// read rather than after the page is already up.
    var art: CoverArtCache?

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
            let rows = await request.detached()
            guard !Task.isCancelled else { return }
            show(rows)
            isLoading = false
        }
    }

    private var request: Request {
        Request(
            section: section, filter: filter, sort: albumSort, seed: shuffleSeed, engine: engine
        )
    }

    /// Publish what came back, and only where it differs from what is already
    /// on screen.
    ///
    /// `@Observable` has no opinion about equality: assigning the same rows
    /// again is still a mutation, and a mutation of a listing is a `ForEach`
    /// diff over every id in it, a layout pass and a commit — 5,610 records and
    /// 7,138 artists on a large library. The same answer as last time is the
    /// common case, not the rare one: every library version bump reloads, so a
    /// download landing or a playlist edit asks again, and so does every return
    /// to a section already visited. Comparing the rows is one walk over them.
    /// Publishing them is thousands of views' worth of work that changes
    /// nothing on screen.
    private func show(_ rows: Rows) {
        switch rows {
        case .none:
            break
        case .albums(let rows):
            if rows != visibleAlbums { visibleAlbums = rows }
        case .artists(let rows):
            if rows != visibleArtists { visibleArtists = rows }
        case .favourites(let tracks, let albums, let artists):
            if tracks != visibleFavourites { visibleFavourites = tracks }
            if albums != visibleFavouriteAlbums { visibleFavouriteAlbums = albums }
            if artists != visibleFavouriteArtists { visibleFavouriteArtists = artists }
        case .history(let rows):
            if rows != visiblePlayHistory { visiblePlayHistory = rows }
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

    /// The record a page is showing, and its tracks, as one value.
    ///
    /// Loaded *before* the page appears — see `Navigator.open(album:)`. A page
    /// that fetches once it is already on screen has to draw itself empty
    /// first, and the empty state of a record page is the word "Album" over
    /// nothing. Both halves land together or not at all, so the header can
    /// never arrive ahead of the rows either.
    private(set) var detailRecord: AlbumRecord?

    struct AlbumRecord: Sendable {
        let albumId: Int64
        /// The library version it was read at. What makes asking for the record
        /// already on screen free, and asking for it after the rows moved a
        /// real read.
        let stamp: UInt64
        var album: Album?
        var tracks: [Track]
    }

    /// Set by `AppState`. Read for the library version a record was loaded at.
    weak var mirror: EngineMirror?

    /// Read the record and its tracks, off the main actor and both at once.
    ///
    /// `.task` and every view callback are main-actor isolated, and isolation
    /// is inherited by every suspension point — so awaiting the engine from one
    /// means the *answer* waits for a main-actor slot to be delivered. It queued
    /// behind the state mirror's batch, which lands every hundred milliseconds:
    /// the engine answered in 300µs and the page saw it a tenth of a second
    /// later, every time, whatever the record. Detached, it comes back in one.
    func prepare(album id: Int64) async {
        let stamp = mirror?.libraryVersion ?? 0
        // Already in hand, and nothing has changed under it. The navigator loads
        // a record before it moves to it, so the page's own `.reloading` asks
        // again the moment it appears — and that second read is identical, lands
        // while the artwork it kicked off is still competing, and takes twenty
        // times what the first one did. A fast page followed by a slow redraw of
        // the same page reads worse than a slow page.
        if let held = detailRecord, held.albumId == id, held.stamp == stamp { return }

        // The sleeve and the colour the room takes from it. Coming from the grid
        // both are already decoded, and the page, its cover and the room's
        // colour go up in one change — see `ArtworkBleed.answered`, which reads
        // them straight through rather than waiting to be handed them.
        //
        // Never waited on. Arriving cold this is an HTTP round trip, and every
        // millisecond spent here is a millisecond the click looks ignored: the
        // navigator holds the page you are leaving on screen until this returns.
        // The room catches up on its own a moment later, which costs a second
        // commit and is the right trade — a page you are already reading.
        warm(album: id)
        let engine = self.engine
        let loaded = await Trace.region("engine-reads") {
            await Task.detached(priority: .userInitiated) {
                let page = try? await engine.albumPage(albumId: id)
                return AlbumRecord(
                    albumId: id,
                    stamp: stamp,
                    album: page?.album,
                    tracks: page?.tracks ?? []
                )
            }.value
        }
        detailRecord = loaded
    }

    /// An artist, their records and who they sound like, as one value.
    ///
    /// The record's shape, for the other page that is about one thing. Loaded
    /// before the page appears — see `Navigator.open(artist:)`.
    private(set) var detailArtist: ArtistRecord?

    struct ArtistRecord: Sendable {
        let artistId: Int64
        /// The library version it was read at, so asking again for the artist
        /// already on screen is free and asking after a scan is a real read.
        let stamp: UInt64
        var artist: Artist?
        var albums: [Album]
        var similar: [SimilarArtist]
    }

    /// Read an artist and everything their page draws, at once and off the main
    /// actor. Three independent queries, so three at once: one after another
    /// each waited on the one before for no reason.
    func prepare(artist id: Int64) async {
        let stamp = mirror?.libraryVersion ?? 0
        if let held = detailArtist, held.artistId == id, held.stamp == stamp { return }

        let engine = self.engine
        detailArtist = await Trace.region("engine-reads") {
            await Task.detached(priority: .userInitiated) {
                async let artist = try? await engine.artist(artistId: id)
                async let albums = try? await engine.albums(
                    artistId: id, sort: .year, seed: 0, search: nil
                )
                async let similar = try? await engine.similarArtists(artistId: id)
                return ArtistRecord(
                    artistId: id,
                    stamp: stamp,
                    artist: await artist ?? nil,
                    albums: await albums ?? [],
                    similar: await similar ?? []
                )
            }.value
        }
    }

    /// Put this record's sleeve and its colour in the cache, if they are not
    /// there already. Detached and never waited on — see the call site.
    private func warm(album id: Int64) {
        guard let art, art.cached(.album(id), size: .tile) == nil else { return }
        Task.detached {
            _ = await art.image(for: .album(id), size: .tile)
            _ = await art.dominantColour(for: .album(id))
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
            // Three independent reads, so three at once. Written as a tuple of
            // awaits they ran one after another, and the second waited on the
            // first for no reason at all.
            async let tracks = engine.favouriteTrackIds()
            async let albums = engine.favouriteAlbumIds()
            async let artists = engine.favouriteArtistIds()
            let trackIds = Set((try? await tracks) ?? [])
            let albumIds = Set((try? await albums) ?? [])
            let artistIds = Set((try? await artists) ?? [])
            // Guarded for the same reason a listing is. Every grid cell and
            // every row reads these to draw its heart, so republishing a set
            // that has not moved redraws the whole page for nothing — and a
            // sync reconciling favourites usually finds them all the same.
            if trackIds != favouriteTrackIds { favouriteTrackIds = trackIds }
            if albumIds != favouriteAlbumIds { favouriteAlbumIds = albumIds }
            if artistIds != favouriteArtistIds { favouriteArtistIds = artistIds }
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
        guard activity?.conflicts(with: [.remoteTracks]) != true else { return }
        let engine = self.engine
        let job = activity?.begin(
            full ? "Full sync with server" : "Syncing with server",
            uses: [.remoteTracks]
        )
        Task {
            _ = try? await engine.syncRemote(full: full)
            if let job { activity?.end(job) }
        }
    }

    /// Throw away every file cached from the server.
    ///
    /// The library rows stay — they are what the server said exists — so the
    /// tracks remain playable and simply download again on demand. It holds the
    /// cached copies and nothing else, so a scan or a sync can carry on beside
    /// it: neither has an opinion about what is on disk in the cache directory.
    func clearDownloads() {
        guard activity?.conflicts(with: [.downloads]) != true else { return }
        let engine = self.engine
        let job = activity?.begin("Clearing downloaded files", uses: [.downloads])
        Task {
            _ = try? await engine.clearDownloadCache()
            if let job { activity?.end(job) }
            loadStats()
        }
    }

    /// Throw away the downloaded copies of these tracks.
    ///
    /// Claims nothing, unlike the library-wide tasks: it touches only the rows
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

    /// Fetch these tracks into the cache without queueing them.
    ///
    /// Tracks already downloaded are skipped, so asking for a record you have
    /// most of costs only the rest of it.
    func downloadToCache(trackIds: [Int64]) {
        guard !trackIds.isEmpty else { return }
        let engine = self.engine
        Task { try? await engine.downloadToCache(trackIds: trackIds) }
    }

    /// Full rescan of every configured folder. Minutes on a big library, so it
    /// runs detached and the UI stays live throughout.
    func scan(force: Bool = false) {
        guard activity?.conflicts(with: .localLibrary) != true else { return }
        scanSummary = nil

        let engine = self.engine
        let job = activity?.begin(
            force ? "Rescanning every file" : "Scanning library",
            uses: .localLibrary,
            cancellable: true
        )
        let progress = job.flatMap { activity?.reporter(for: $0) }
        Task {
            let result = try? await engine.scanReporting(force: force, reporter: progress)
            if let job { activity?.end(job) }
            scanSummary = result
        }
    }

    /// Rows appeared or vanished underneath us — a scan, a sync, an import, a
    /// playlist edit, a download landing, a folder being forgotten. Whether
    /// this app asked for it or the engine did it on its own makes no
    /// difference here: nothing to merge, nothing to invalidate, just ask
    /// again.
    ///
    /// Favourites too, because a sync reconciles them with the server and the
    /// hearts on screen are stale the moment it lands.
    ///
    /// The section's rows only. What a *page* is showing reloads where it is
    /// drawn — see `View.reloading(on:)`.
    func libraryChanged() {
        loadStats()
        refreshFavourites()
        reload()
    }
}

/// Everything a section's query depends on, captured off the model so the
/// answer that lands belongs to the question that was asked. Anything that
/// changes one cancels the task holding it.
private struct Request: Sendable {
    let section: Navigator.Section
    let filter: String
    let sort: AlbumSort
    let seed: Int64
    let engine: KoanEngine

    var search: String? { filter.isEmpty ? nil : filter }

    /// Everything this section is showing, read off the main actor.
    ///
    /// Detached because isolation is inherited by every suspension point: await
    /// the engine from a main-actor task and the *answer* waits for a
    /// main-actor slot to be delivered, behind whatever the state mirror is
    /// applying. The read is microseconds; the wait for the hop was not.
    func detached() async -> Rows {
        await Task.detached(priority: .userInitiated) { await self.rows() }.value
    }

    /// Everything this section is showing.
    private func rows() async -> Rows {
        switch section {
        case .queue, .searchResults, .playlist, .downloads:
            // Owned by the player, search, playlist and downloads models
            // respectively.
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

private enum Rows: Sendable {
    case none
    case albums([Album])
    case artists([Artist])
    case favourites(tracks: [Track], albums: [Album], artists: [Artist])
    case history([PlayHistoryEntry])
}
