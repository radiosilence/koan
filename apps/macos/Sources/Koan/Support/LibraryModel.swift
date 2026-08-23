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
            load()
        }
    }

    /// Substring filter over whatever the current section is showing.
    var filter: String = ""

    /// Newest first by default: the record you just added is the one you're
    /// looking for. Persisted so it survives a relaunch.
    var albumSort: AlbumSort = .recentlyAdded {
        didSet {
            guard albumSort != oldValue else { return }
            UserDefaults.standard.set(albumSort.storageKey, forKey: "albumSort")
            reloadAlbums()
        }
    }

    private(set) var albums: [Album] = []
    private(set) var artists: [Artist] = []
    private(set) var favourites: [Track] = []
    private(set) var snapshots: [Snapshot] = []
    private(set) var stats: Stats?
    private(set) var isLoading = false

    /// The browse stack's path. Lives here so search can push a destination
    /// rather than each browser owning a stack nothing else can reach.
    var path = NavigationPath()

    var selectedArtistId: Int64?
    var selectedAlbumId: Int64?
    private(set) var detailTracks: [Track] = []

    /// Set while a scan runs so the UI can show progress and refuse a second one.
    private(set) var isScanning = false
    var scanSummary: ScanSummary?

    init(engine: KoanEngine) {
        self.engine = engine
        if let stored = UserDefaults.standard.string(forKey: "albumSort"),
           let sort = AlbumSort(storageKey: stored) {
            albumSort = sort
        }
    }

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

    var visibleAlbums: [Album] {
        let scoped = selectedArtistId.map { id in albums.filter { $0.artistId == id } } ?? albums
        guard !filter.isEmpty else { return scoped }
        return scoped.filter {
            $0.title.localizedCaseInsensitiveContains(filter)
                || $0.artistName.localizedCaseInsensitiveContains(filter)
        }
    }

    var visibleArtists: [Artist] {
        guard !filter.isEmpty else { return artists }
        return artists.filter { $0.name.localizedCaseInsensitiveContains(filter) }
    }

    var visibleFavourites: [Track] {
        guard !filter.isEmpty else { return favourites }
        return favourites.filter {
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

    /// Jump straight to a thing from search: switch to the section it lives in,
    /// then push its detail view.
    /// The track that search sent you here for, so the album view can single it
    /// out. Cleared once the view has scrolled to it.
    var highlightedTrackId: Int64?

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
    }

    func reveal(artist id: Int64) {
        path.append(ArtistRoute(id: id))
    }

    // MARK: - Mutations

    /// Refresh only what a favourite toggle can change, rather than reloading
    /// the section wholesale.
    func refreshFavourites() {
        let engine = self.engine
        Task {
            let updated = await Task.detached(priority: .utility) {
                (try? engine.favourites()) ?? []
            }.value
            favourites = updated
            if let albumId = selectedAlbumId { loadTracks(albumId: albumId) }
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

    /// Full rescan of every configured folder. Minutes on a big library, so it
    /// runs detached and the UI stays live throughout.
    func scan(force: Bool = false) {
        guard !isScanning else { return }
        isScanning = true
        scanSummary = nil

        let engine = self.engine
        Task {
            let result = await Task.detached(priority: .utility) {
                try? engine.scan(force: force)
            }.value
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
