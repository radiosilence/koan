import Foundation
import KoanFFI

/// Global search across artists, albums and tracks.
///
/// One query, three result sets — the shape every streaming client uses, and
/// the reason there is no separate per-view filter: two search boxes competing
/// for the same intent is worse than one that always means the same thing.
///
/// Tracks go through FTS5 and artists/albums through nucleo. That split is
/// deliberate: fuzzy matching rebuilds its corpus from every row it searches,
/// which is fine across a few thousand artists and wasteful across fifty
/// thousand tracks on every keystroke.
@MainActor
@Observable
final class SearchModel {
    private let engine: KoanEngine
    private let library: LibraryModel

    var query: String = ""

    private(set) var artists: [Artist] = []
    private(set) var albums: [Album] = []
    private(set) var tracks: [Track] = []
    private(set) var isSearching = false

    private var task: Task<Void, Never>?

    init(engine: KoanEngine, library: LibraryModel) {
        self.engine = engine
        self.library = library
    }

    var isEmpty: Bool { artists.isEmpty && albums.isEmpty && tracks.isEmpty }
    var hasQuery: Bool { !query.trimmingCharacters(in: .whitespaces).isEmpty }

    /// What the dropdown shows — enough to recognise the thing you meant,
    /// not enough to browse.
    var quickArtists: [Artist] { Array(artists.prefix(3)) }
    var quickAlbums: [Album] { Array(albums.prefix(3)) }
    var quickTracks: [Track] { Array(tracks.prefix(5)) }

    /// Debounced: a fast typist would otherwise queue a round of queries per
    /// character, and the fuzzy passes are the expensive half.
    func schedule() {
        task?.cancel()
        let text = query.trimmingCharacters(in: .whitespaces)
        guard !text.isEmpty else {
            clear()
            return
        }

        isSearching = true
        let engine = self.engine
        task = Task {
            try? await Task.sleep(for: .milliseconds(160))
            guard !Task.isCancelled else { return }

            let found = await Task.detached(priority: .userInitiated) {
                (
                    (try? engine.search(query: text, limit: 60)) ?? [],
                    (try? engine.fuzzySearch(query: text, kind: .album, limit: 30)) ?? [],
                    (try? engine.fuzzySearch(query: text, kind: .artist, limit: 30)) ?? []
                )
            }.value

            guard !Task.isCancelled else { return }
            tracks = found.0
            // Fuzzy matching answers with ids; the objects come from the
            // library caches, which are loaded once at launch.
            albums = found.1.compactMap { library.album(id: $0.id) }
            artists = found.2.compactMap { library.artist(id: $0.id) }
            isSearching = false
        }
    }

    func clear() {
        task?.cancel()
        artists = []
        albums = []
        tracks = []
        isSearching = false
    }

    func reset() {
        query = ""
        clear()
    }
}
