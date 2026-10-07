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
    /// The album browser's pick and the artist page's, each over its own grid.
    /// Unobserved, so reaching one subscribes nothing; what it holds is
    /// observed where it is drawn.
    @ObservationIgnored private(set) lazy var selection = PlayableSelection { [unowned self] in
        visibleAlbums.map(Playable.album)
    }
    @ObservationIgnored private(set) lazy var artistSelection = PlayableSelection { [unowned self] in
        (detailArtist?.albums ?? []).map(Playable.album)
    }

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
        /// The name filter it was read with: empty, unless a shelf's heading carried
        /// one in.
        let filter: String
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
        // invisible here, and an apparently empty library is the result. The
        // one exception is a shelf's heading, which arrives with the shelf's query.
        let filter = carried ?? ""
        carried = nil
        return await Listing(
            section: section,
            filter: filter,
            rows: Request(
                section: section, filter: filter, browse: browseFilter, sort: albumSort,
                trackSort: trackSort, seed: shuffleSeed, engine: engine
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
        filter = listing.filter
        adopting = false
        show(listing.rows)
        isLoading = false
        continueTracks()
    }

    // MARK: - Shelves

    /// Which listing a shelf section opens.
    enum ShelfList {
        case artists, albums, tracks

        var section: Section {
            switch self {
            case .artists: .artists
            case .albums: .albums
            case .tracks: .tracks
            }
        }
    }

    /// The name filter the next prepared section is read with. Set by a See
    /// all, and spent by the move it makes.
    @ObservationIgnored private var carried: String?

    /// Point a browser at a shelf: its filters and sort set to the shelf's, so
    /// the listing it opens on is the one the shelf's preview is the head of
    /// and its count is the one the shelf gave. Returns the section to show.
    ///
    /// The filters replace whatever was set, as a link does, and stay set,
    /// shown in the filter control and cleared from it like any other.
    func browse(_ list: ShelfList, of shelf: ShelfKind) -> Section {
        seeding = true
        defer { seeding = false }
        var filter = BrowseFilter.none
        switch shelf {
        case .favourites:
            filter.favourites = true
            albumSort = .artist
            trackSort = .artist
        case .recent:
            filter.recent = true
            albumSort = .lastPlayed
            trackSort = .lastPlayed
        case .search(let query):
            carried = query
            albumSort = .bestMatch
            trackSort = .bestMatch
        case .downloaded:
            filter.downloaded = true
            albumSort = .downloaded
            trackSort = .artist
        }
        browseFilter = filter
        return list.section
    }

    /// True while a shelf's heading sets the browser up, which is one change and asks
    /// for nothing until the move it precedes.
    @ObservationIgnored private var seeding = false

    /// Substring filter over whatever the current section is showing. It
    /// narrows the query, not the answer.
    var filter = "" {
        didSet {
            guard filter != oldValue, !adopting else { return }
            reload(debounced: true)
        }
    }

    /// True while a prepared listing is being adopted — see `show(_:)`.
    private var adopting = false

    /// Long enough that a burst of typing is one round trip, short enough not
    /// to read as lag.
    private static let filterDebounce = Duration.milliseconds(80)

    /// Newest first by default: the record you just added is the one you're
    /// looking for. Persisted so it survives a relaunch.
    var albumSort: AlbumSort = .recentlyAdded {
        didSet {
            guard albumSort != oldValue else { return }
            UserDefaults.standard.set(albumSort.storageKey, forKey: "albumSort")
            guard !seeding else { return }
            reload()
        }
    }

    /// What the album and artist browsers are narrowed to beyond the name
    /// filter. Unlike the name filter it follows you between the two and
    /// survives a relaunch, as the sort does: it is a standing choice, and the
    /// control showing how many are on says so.
    var browseFilter: BrowseFilter = .none {
        didSet {
            guard browseFilter != oldValue else { return }
            UserDefaults.standard.set(browseFilter.stored, forKey: "browseFilter")
            guard !seeding, section.isBrowser else { return }
            reload()
        }
    }

    /// How the track browser is ordered. Persisted as the album sort is.
    var trackSort: TrackBrowseSort = .artist {
        didSet {
            guard trackSort != oldValue else { return }
            UserDefaults.standard.set(trackSort.storageKey, forKey: "trackSort")
            guard !seeding, section == .tracks else { return }
            reload()
        }
    }

    /// Whether the browser on screen is showing less than the whole library.
    var isNarrowed: Bool { !filter.isEmpty || browseFilter.activeCount > 0 }

    /// What the codec and genre filters offer, read when the filters open.
    private(set) var browseChoices: BrowseChoices?

    func loadBrowseChoices() async {
        let engine = self.engine
        let choices = await Task.detached { try? await engine.browseChoices() }.value
        if let choices { browseChoices = choices }
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

    /// Where each browser was scrolled to when it was left, so it can be rebuilt
    /// on the way back and put where it was. Not observed: nothing redraws
    /// because of them.
    @ObservationIgnored var albumsOffset: CGFloat?
    /// The Mac's artist list, which is a table and remembers a distance as the
    /// album grid does. The phone's list goes back to a row instead — below.
    @ObservationIgnored var artistsOffset: CGFloat?
    @ObservationIgnored var artistsTop: Int64?
    /// The artist rows on screen, kept as they come and go so the top one can
    /// be read when the list is left.
    @ObservationIgnored var artistsShown: Set<Int64> = []
    /// How many rows above the visible ones the list keeps ready, so the top
    /// of what was on screen can be told from the top of what was built.
    @ObservationIgnored var artistsOverscan = 0

    /// What the section on screen is showing, as the database handed it over.
    /// Stored rather than computed because a `List` reads its collection far
    /// more than once per update, and anything derived on read is derived a few
    /// hundred times a frame.
    private(set) var visibleAlbums: [Album] = []
    private(set) var visibleArtists: [Artist] = []
    private(set) var visiblePlayHistory: [PlayHistoryEntry] = []
    /// The shelf on screen: Favourites' or Recently Played's previews and
    /// how many there are of each.
    private(set) var visibleShelf: ShelfSummary?
    /// The track browser's rows so far, and how many it will have. Read a page
    /// at a time — see `continueTracks()`.
    private(set) var visibleTracks: [Track] = []
    private(set) var trackTotal: UInt64 = 0

    // Favourite state is read from here rather than from the copy baked into
    // each Track when it was fetched. A track appears in the album view, the
    // artist view, the queue, the picker and search results, and refetching
    // every one of those after a heart click is neither cheap nor reliable.
    private(set) var favouriteTrackIds: Set<Int64> = []
    private(set) var favouriteAlbumIds: Set<Int64> = []
    private(set) var favouriteArtistIds: Set<Int64> = []

    func isFavourite(track id: Int64) -> Bool { favouriteTrackIds.contains(id) }
    func isFavourite(album id: Int64) -> Bool { favouriteAlbumIds.contains(id) }
    func isFavourite(artist id: Int64) -> Bool { favouriteArtistIds.contains(id) }

    private(set) var stats: Stats?
    /// Whether a server and its credential are set, loaded with `stats`: an
    /// empty library means something different signed in and signed out.
    private(set) var signedIn: Bool?
    private(set) var isLoading = false

    /// What an empty page says on a phone or a television, whose library is a
    /// server's.
    var emptyLibraryDetail: String {
        signedIn == true
            ? "Nothing from your server yet. It appears here once kōan has synced; Settings → Server shows how that is going."
            : "Sign in to your music server in Settings → Server."
    }

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
        if let stored = UserDefaults.standard.dictionary(forKey: "browseFilter") {
            browseFilter = BrowseFilter(stored: stored)
        }
        if let stored = UserDefaults.standard.string(forKey: "trackSort"),
           let sort = TrackBrowseSort(storageKey: stored) {
            trackSort = sort
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
            continueTracks()
        }
    }

    private var request: Request {
        Request(
            section: section, filter: filter, browse: browseFilter, sort: albumSort,
            trackSort: trackSort, seed: shuffleSeed, engine: engine
        )
    }

    /// The rest of the track browser, a page at a time after the first, each
    /// appended as it lands. Part of the load it follows: anything that
    /// reloads cancels it, so a page of the old listing never lands on the new.
    private func continueTracks() {
        guard section == .tracks, UInt64(visibleTracks.count) < trackTotal else { return }
        let request = self.request
        let from = visibleTracks.count
        loading = Task {
            var offset = from
            while !Task.isCancelled, UInt64(offset) < trackTotal {
                guard let page = await request.tracks(offset: UInt32(offset)), !page.tracks.isEmpty
                else { return }
                guard !Task.isCancelled else { return }
                visibleTracks.append(contentsOf: page.tracks)
                offset += page.tracks.count
            }
        }
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
        case .shelf(let shelf):
            if shelf != visibleShelf { visibleShelf = shelf }
        case .history(let rows):
            if rows != visiblePlayHistory { visiblePlayHistory = rows }
        case .tracks(let listing):
            if listing.total != trackTotal { trackTotal = listing.total }
            if listing.tracks != visibleTracks { visibleTracks = listing.tracks }
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
            signedIn = await engine.settings().remoteSignedIn
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
    /// means the *answer* waits for a main-actor slot to be delivered, behind
    /// whatever the state mirror is applying. Detached, it does not wait.
    func prepare(album id: Int64) async {
        let stamp = mirror?.libraryVersion ?? 0
        // Already in hand, and nothing has changed under it. The navigator loads
        // a record before it moves to it, so the page's own `.reloading` asks
        // again the moment it appears — and that second read is identical, lands
        // while the artwork it kicked off is still competing, and takes twenty
        // times what the first one did. A fast page followed by a slow redraw of
        // the same page reads worse than a slow page.
        if let held = detailRecord, held.albumId == id, held.stamp == stamp { return }

        // The sleeve and the colour the room takes from it. Once both are
        // decoded the page, its cover and the room's colour go up in one
        // change — see `ArtworkBleed.answered`, which reads them straight
        // through rather than waiting to be handed them.
        //
        // Never waited on. Arriving cold this is an HTTP round trip, and every
        // millisecond spent here is a millisecond the click looks ignored: the
        // navigator holds the page you are leaving on screen until this returns.
        // The room catches up on its own a moment later, which costs a second
        // commit and is the right trade — a page you are already reading.
        warm(.album(id))
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
        /// For an artist who owns no albums (a guest, a compilation track):
        /// the tracks credited to them, which are then the whole page.
        var appearances: [Track] = []
        /// Biography and photograph, as cached. Filled in from the network
        /// after the page is up — see `enrich(artist:)`.
        var info: ArtistInfo?
        /// A network lookup is under way and nothing is cached to show yet.
        var infoLoading = false
    }

    /// Read an artist and everything their page draws, at once and off the main
    /// actor. Independent queries, so all at once.
    func prepare(artist id: Int64) async {
        let stamp = mirror?.libraryVersion ?? 0
        if let held = detailArtist, held.artistId == id, held.stamp == stamp { return }

        let engine = self.engine
        detailArtist = await Trace.region("engine-reads") {
            await Task.detached(priority: .userInitiated) {
                async let artist = try? await engine.artist(artistId: id)
                async let albums = try? await engine.albums(
                    artistId: id, sort: .year, seed: 0, search: nil, filter: .none
                )
                async let info = try? await engine.artistInfo(artistId: id)
                let cached = await info ?? nil
                let owned = await albums ?? []
                // Read only when there is nothing else to show: an artist
                // with albums has their tracks one tap away on each.
                let appearances =
                    owned.isEmpty
                    ? (try? await engine.tracks(
                        albumId: nil, artistId: id, sort: .album, limit: 500, offset: 0
                    )) ?? []
                    : []
                return ArtistRecord(
                    artistId: id,
                    stamp: stamp,
                    artist: await artist ?? nil,
                    albums: owned,
                    appearances: appearances,
                    info: cached,
                    infoLoading: cached == nil
                )
            }.value
        }
        enrich(artist: id)
    }

    /// Ask for the artist's biography and photograph, and fold them into the
    /// page when they land.
    ///
    /// Never waited on: a miss is several seconds of MusicBrainz and Wikipedia,
    /// and the page is already drawn from the cache. A fresh cache answers at
    /// once and changes nothing.
    private func enrich(artist id: Int64) {
        let engine = self.engine
        Task {
            let fetched = try? await engine.fetchArtistInfo(artistId: id)
            guard detailArtist?.artistId == id else { return }
            detailArtist?.infoLoading = false
            guard let fetched, detailArtist?.info != fetched else { return }
            detailArtist?.info = fetched
        }
    }

    /// Put this record's sleeve and its colour in the cache, if they are not
    /// there already. Detached and never waited on — see `prepare(album:)`.
    ///
    /// Each on its own account: a grid has drawn the sleeve of every record
    /// in it and worked out the colour of none of them.
    func warm(_ source: AlbumArtwork.Source) {
        guard let art else { return }
        let needsImage = art.cached(source, size: .tile) == nil
        let needsColour = art.cachedColour(for: source) == nil
        guard needsImage || needsColour else { return }
        Task.detached {
            if needsImage { _ = await art.image(for: source, size: .tile) }
            if needsColour { _ = await art.dominantColour(for: source) }
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
            guard let now = try? await engine.toggleFavourite(trackId: id) else { return }
            if now { favouriteTrackIds.insert(id) } else { favouriteTrackIds.remove(id) }
            reloadFavourites()
        }
    }

    func toggleFavourite(album id: Int64) {
        let engine = self.engine
        Task {
            guard let now = try? await engine.toggleFavouriteAlbum(albumId: id) else { return }
            if now { favouriteAlbumIds.insert(id) } else { favouriteAlbumIds.remove(id) }
            reloadFavourites()
        }
    }

    func toggleFavourite(artist id: Int64) {
        let engine = self.engine
        Task {
            guard let now = try? await engine.toggleFavouriteArtist(artistId: id) else { return }
            if now { favouriteArtistIds.insert(id) } else { favouriteArtistIds.remove(id) }
            reloadFavourites()
        }
    }

    /// Re-read every favourite id from the database. Called after a sync, which
    /// can change them without going through a toggle.
    func refreshFavourites() {
        let engine = self.engine
        Task {
            // Three independent reads, so three at once.
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
        guard section == .favourites || (section.isBrowser && browseFilter.favourites) else { return }
        reload()
    }

    /// Pull the remote library. Minutes on a large server, so it runs detached.
    /// Nothing here refreshes anything: the engine announces the rows it wrote,
    /// and `libraryChanged()` runs off that.
    func syncRemote() {
        guard activity?.conflicts(with: [.remoteTracks]) != true else { return }
        let engine = self.engine
        let job = activity?.begin(
            "Syncing with server",
            uses: [.remoteTracks],
            followsSync: true
        )
        Task {
            _ = try? await engine.syncRemote()
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

    /// A play was recorded or forgotten: the sections derived from history
    /// ask again, and nothing else does.
    func historyChanged() {
        switch section {
        case .recentlyPlayed, .playHistory: reload()
        case let section where section.isBrowser && browseFilter.recent: reload()
        default: break
        }
    }
}

/// How many artists, records and tracks a shelf has in all, beside the
/// first few it shows: what its headings say.
struct ShelfTotals: Equatable {
    let artists: UInt64
    let albums: UInt64
    let tracks: UInt64

    init(_ summary: ShelfSummary) {
        artists = summary.artistTotal
        albums = summary.albumTotal
        tracks = summary.trackTotal
    }
}

/// Everything a section's query depends on, captured off the model so the
/// answer that lands belongs to the question that was asked. Anything that
/// changes one cancels the task holding it.
private struct Request: Sendable {
    let section: Navigator.Section
    let filter: String
    let browse: BrowseFilter
    let sort: AlbumSort
    let trackSort: TrackBrowseSort
    let seed: Int64
    let engine: KoanEngine

    /// How many tracks the browser reads at once.
    static let trackPage: UInt32 = 1000

    /// A page of the track browser.
    func tracks(offset: UInt32) async -> TrackListing? {
        await Task.detached(priority: .userInitiated) {
            try? await engine.trackListing(
                sort: trackSort, search: search, filter: browse, limit: Self.trackPage, offset: offset
            )
        }.value
    }

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
                    artistId: nil, sort: sort, seed: seed, search: search, filter: browse
                )) ?? []
            )
        case .artists:
            return .artists((try? await engine.artists(search: search, filter: browse)) ?? [])
        case .tracks:
            return (try? await engine.trackListing(
                sort: trackSort, search: search, filter: browse, limit: Self.trackPage, offset: 0
            )).map { .tracks($0) } ?? .none
        case .favourites:
            return (try? await engine.shelfSummary(shelf: .favourites)).map { .shelf($0) } ?? .none
        case .playHistory:
            return .history((try? await engine.playHistory(search: search)) ?? [])
        case .recentlyPlayed:
            return (try? await engine.shelfSummary(shelf: .recent)).map { .shelf($0) } ?? .none
        case .onDevice:
            return (try? await engine.shelfSummary(shelf: .downloaded)).map { .shelf($0) } ?? .none
        }
    }
}

private enum Rows: Sendable {
    case none
    case albums([Album])
    case artists([Artist])
    case shelf(ShelfSummary)
    case history([PlayHistoryEntry])
    case tracks(TrackListing)
}
