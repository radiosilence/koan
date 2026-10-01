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
    private let nav: Navigator

    /// Searching follows from the query changing, here rather than in an
    /// `onChange` on a view: a view that watches the query is a view that is
    /// rebuilt on every keystroke.
    var query: String = "" {
        didSet {
            guard query != oldValue else { return }
            let has = !query.trimmingCharacters(in: .whitespaces).isEmpty
            if has != hasQuery { hasQuery = has }
            schedule()
        }
    }

    /// Stored, and only written when it flips. Computed from `query`, it would
    /// make everything that asked — the sidebar, for its Results row — a reader
    /// of every keystroke.
    private(set) var hasQuery = false

    private(set) var artists: [Artist] = []
    private(set) var albums: [Album] = []
    private(set) var tracks: [Track] = []
    private(set) var isSearching = false

    /// A pick across the results, held through new queries — see
    /// `PlayableSelection`. Unobserved, like the library's.
    @ObservationIgnored private(set) lazy var selection = PlayableSelection { [unowned self] in
        artists.map { .artist(id: $0.id, name: $0.name) }
            + albums.map(Playable.album)
            + tracks.map(Playable.track)
    }

    private var task: Task<Void, Never>?
    /// Where to put the user back when they clear the field — the whole
    /// location, so a detail view you searched from is still there afterwards.
    private var locationBeforeSearch: Navigator.Page?

    init(engine: KoanEngine, nav: Navigator) {
        self.engine = engine
        self.nav = nav
    }

    var isEmpty: Bool { artists.isEmpty && albums.isEmpty && tracks.isEmpty }

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

        // Where to put them back, captured now — the move itself waits for
        // results, and by then this is no longer where they were.
        if nav.section != .searchResults {
            locationBeforeSearch = nav.current
        }

        // Only on the edge: written per keystroke, the results page would re-run
        // per keystroke for a flag that had not moved.
        if !isSearching { isSearching = true }
        let engine = self.engine
        task = Task {
            try? await Task.sleep(for: .milliseconds(160))
            guard !Task.isCancelled else { return }

            // Rows, not ids: the engine already read them to rank them, and
            // resolving ids would mean holding a catalogue to resolve against.
            let found = (
                (try? await engine.search(query: text, limit: 60)) ?? [],
                (try? await engine.fuzzyAlbums(query: text, limit: 30)) ?? [],
                (try? await engine.fuzzyArtists(query: text, limit: 30)) ?? []
            )

            guard !Task.isCancelled else { return }
            tracks = found.0
            albums = found.1
            artists = found.2
            isSearching = false
            // Moved once there is something to show: the page you were on is a
            // better thing to look at while the answer is being read than an
            // empty results page.
            nav.show(.searchResults)
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
        // Forgotten first: emptying the query runs `schedule`, which would
        // otherwise take them back to where they searched from.
        locationBeforeSearch = nil
        query = ""
        clear()
    }
    /// Return either picks a suggestion — in which case the field holds a token
    /// naming exactly what was chosen — or it means "show me everything".
    ///
    /// On the model rather than in a scene root, for the same reason `schedule`
    /// is: a shell that offers a search field should not also have to know what
    /// submitting one means.
    func submit() {
        // Emptying the field submits it again. Acting on that would open the
        // results page for a search nobody asked for — and since clearing the
        // query then forgets that page, you would land on whatever list was
        // behind it, one keystroke after picking an album.
        let text = query.trimmingCharacters(in: .whitespaces)
        guard !text.isEmpty else { return }

        guard let selection = Selection(token: text) else {
            nav.show(.searchResults)
            return
        }
        switch selection {
        case .track(let id, let albumId):
            // A track lives on its album; that's where you'd play it from.
            if let albumId { nav.open(album: albumId, highlighting: id) }
        case .album(let id):
            nav.open(album: id)
        case .artist(let id):
            nav.open(artist: id)
        }
        reset()
    }

}
