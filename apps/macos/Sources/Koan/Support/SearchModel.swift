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
    private let nav: Navigator

    var query: String = ""

    private(set) var artists: [Artist] = []
    private(set) var albums: [Album] = []
    private(set) var tracks: [Track] = []
    private(set) var isSearching = false

    private var task: Task<Void, Never>?
    /// Where to put the user back when they clear the field — the whole
    /// location, so a detail view you searched from is still there afterwards.
    private var locationBeforeSearch: Location?

    init(engine: KoanEngine, library: LibraryModel, nav: Navigator) {
        self.engine = engine
        self.library = library
        self.nav = nav
    }

    var isEmpty: Bool { artists.isEmpty && albums.isEmpty && tracks.isEmpty }
    var hasQuery: Bool { !query.trimmingCharacters(in: .whitespaces).isEmpty }

    /// What a suggestion row completes to.
    ///
    /// `searchSuggestions` rows can only hand back a *string* — they complete
    /// the query, they don't carry actions. Encoding the kind and id into that
    /// string is what lets submit route to the exact thing that was clicked
    /// instead of just running the text as a search.
    enum Selection {
        case track(Int64, album: Int64?)
        case album(Int64)
        case artist(Int64)

        var token: String {
            switch self {
            case .track(let id, let album): "koan://track/\(id)/\(album.map(String.init) ?? "")"
            case .album(let id): "koan://album/\(id)"
            case .artist(let id): "koan://artist/\(id)"
            }
        }

        init?(token: String) {
            guard token.hasPrefix("koan://") else { return nil }
            let parts = token.dropFirst("koan://".count).split(separator: "/", omittingEmptySubsequences: false)
            switch (parts.first, parts.dropFirst().first.flatMap { Int64($0) }) {
            case ("track", .some(let id)):
                self = .track(id, album: parts.dropFirst(2).first.flatMap { Int64($0) })
            case ("album", .some(let id)):
                self = .album(id)
            case ("artist", .some(let id)):
                self = .artist(id)
            default:
                return nil
            }
        }
    }

    /// True while the field holds a completion token rather than something a
    /// human typed, so we don't fire a search for it.
    var queryIsToken: Bool { Selection(token: query) != nil }

    /// Results land in the main area as you type, rather than in a dropdown.
    /// A floating suggestion list can't carry actions — `searchSuggestions`
    /// exists to complete the query text, not to navigate — and it fought the
    /// lyrics inspector for the same corner of the window.
    ///
    /// Debounced: a fast typist would otherwise queue a round of queries per
    /// keystroke, and the fuzzy passes are the expensive half.
    func schedule() {
        task?.cancel()
        // A completion token isn't a search term; submit will consume it.
        guard !queryIsToken else { return }
        let text = query.trimmingCharacters(in: .whitespaces)
        guard !text.isEmpty else {
            clear()
            if let previous = locationBeforeSearch {
                nav.go(to: previous)
                locationBeforeSearch = nil
            }
            return
        }

        if nav.section != .searchResults {
            locationBeforeSearch = nav.location
            nav.show(.searchResults)
        }

        isSearching = true
        let engine = self.engine
        task = Task {
            try? await Task.sleep(for: .milliseconds(160))
            guard !Task.isCancelled else { return }

            let found = (
                (try? await engine.search(query: text, limit: 60)) ?? [],
                (try? await engine.fuzzySearch(query: text, kind: .album, limit: 30)) ?? [],
                (try? await engine.fuzzySearch(query: text, kind: .artist, limit: 30)) ?? []
            )

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
        // The results page only exists while there is a query, so it stops
        // being somewhere Back can return to.
        nav.forget(.searchResults)
        artists = []
        albums = []
        tracks = []
        isSearching = false
    }

    /// Clearing after acting on a result: the field empties but the user has
    /// already been sent somewhere, so don't drag them back.
    func reset() {
        query = ""
        clear()
        locationBeforeSearch = nil
    }
}
