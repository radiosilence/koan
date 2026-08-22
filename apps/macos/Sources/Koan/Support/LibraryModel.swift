import Foundation
import KoanFFI

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
        case albums
        case artists
        case favourites
        case snapshots
    }

    let engine: KoanEngine

    var section: Section = .queue {
        didSet { if section != oldValue { load() } }
    }

    /// Substring filter over whatever the current section is showing.
    var filter: String = ""

    private(set) var albums: [Album] = []
    private(set) var artists: [Artist] = []
    private(set) var favourites: [Track] = []
    private(set) var snapshots: [Snapshot] = []
    private(set) var stats: Stats?
    private(set) var isLoading = false

    var selectedArtistId: Int64?
    var selectedAlbumId: Int64?
    private(set) var detailTracks: [Track] = []

    /// Set while a scan runs so the UI can show progress and refuse a second one.
    private(set) var isScanning = false
    var scanSummary: ScanSummary?

    init(engine: KoanEngine) {
        self.engine = engine
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
    }

    /// Loads whatever the current section needs. Everything heavy happens off
    /// the main actor; only the assignment comes back.
    func load() {
        let engine = self.engine
        let section = self.section
        isLoading = true

        Task {
            switch section {
            case .queue:
                break  // the player model already owns this
            case .albums:
                if albums.isEmpty {
                    albums = await Task.detached(priority: .userInitiated) {
                        (try? engine.albums(artistId: nil)) ?? []
                    }.value
                }
            case .artists:
                if artists.isEmpty {
                    artists = await Task.detached(priority: .userInitiated) {
                        (try? engine.artists(search: nil)) ?? []
                    }.value
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

    func select(album: Album) {
        selectedAlbumId = album.id
        loadTracks(albumId: album.id)
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
