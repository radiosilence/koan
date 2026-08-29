import SwiftUI

/// The one owner of where you are.
///
/// koan navigates like a browser, not like a hierarchy. An album is reachable
/// from the queue, from the grid, from an artist, from search, and none of
/// those is its parent — so there is no tree to walk and nothing to be "up".
/// What there is, is a list of pages you have been to and a cursor into it.
///
/// This deliberately does not use a `NavigationStack`. A stack navigates by
/// owning a root and a path below it, discards that path whenever the root
/// changes, and writes the empty path back through its binding — which, with a
/// root that switched per section, silently undid any move that changed both at
/// once. Back and forward were already this class's job, and the stack's own
/// back button was already hidden, so it was navigating nothing and charging a
/// hierarchy for it.
///
/// The library follows the location rather than the other way round: what is
/// loaded is a consequence of where you are.
@MainActor
@Observable
final class Navigator {
    /// A row in the sidebar. Distinct from a page only in that these are the
    /// ones you can click to.
    enum Section: Hashable, Identifiable {
        case queue
        case searchResults
        case albums
        case artists
        case favourites
        case playHistory
        case downloads
        /// One playlist. A sidebar row like any other — which is what makes
        /// clicking it light it up, and what lets Back return to it.
        case playlist(Int64)

        var id: Self { self }

        /// What the toolbar's filter field says — and, by its absence, which
        /// sections have no filter at all. The field and ⌘F both read it, so
        /// they cannot disagree about where narrowing is possible.
        var filterPlaceholder: String? {
            switch self {
            case .albums: "Filter albums"
            case .artists: "Filter artists"
            case .favourites: "Filter favourites"
            case .playHistory: "Filter history"
            // Short, and ordered by what is happening rather than by name.
            case .downloads: nil
            // A playlist is a sequence someone chose, and narrowing it hides
            // part of that sequence rather than telling you anything.
            case .queue, .searchResults, .playlist: nil
            }
        }
    }

    /// One page. Everywhere you can be, in one value, with nothing beside it.
    enum Page: Hashable {
        case section(Section)
        case album(Int64)
        case artist(Int64)

        /// The sidebar row this page *is*, if it is one. A record or an artist
        /// is not a row, so on those the sidebar lights nothing — which is the
        /// truth, and is also what makes clicking a row from one of them a move
        /// rather than a no-op.
        var section: Section? {
            if case .section(let section) = self { section } else { nil }
        }
    }

    private(set) var current: Page = .section(.queue)

    /// The track a move was aimed at, so the album view can single it out
    /// rather than dropping you at the top of a twenty-track record. Cleared by
    /// the view once it has scrolled to it.
    var highlightedTrackId: Int64?

    /// Every page actually reached, in the order reached, with a cursor.
    /// Wandering `queue → album → artist → album` is four entries, not four
    /// levels of anything.
    private var history: [Page] = [.section(.queue)]
    private var cursor = 0
    /// The move being loaded, if there is one.
    private var moving: Task<Void, Never>?

    private let library: LibraryModel
    /// Set by `AppState`. A playlist is a page like any other, and its rows are
    /// read before the move like any other page's.
    weak var playlists: PlaylistsModel?

    init(library: LibraryModel) {
        self.library = library
    }

    var section: Section? { current.section }
    var canGoBack: Bool { cursor > 0 }
    var canGoForward: Bool { cursor < history.count - 1 }

    // MARK: - Moves

    func show(_ section: Section) {
        go(to: .section(section))
    }

    func open(album id: Int64, highlighting trackId: Int64? = nil) {
        FrameTimer.shared.begin()
        // Set before the move rather than on arrival: the page that reads it is
        // not on screen yet, and this is the same click.
        highlightedTrackId = trackId
        go(to: .album(id))
    }

    func open(artist id: Int64) {
        go(to: .artist(id))
    }

    func open(playlist id: Int64) {
        go(to: .section(.playlist(id)))
    }

    func goBack() {
        guard canGoBack else { return }
        let landing = cursor - 1
        move(to: history[landing]) { [weak self] in self?.cursor = landing }
    }

    func goForward() {
        guard canGoForward else { return }
        let landing = cursor + 1
        move(to: history[landing]) { [weak self] in self?.cursor = landing }
    }

    /// Go to a page, recording it. The only way anything moves.
    func go(to next: Page) {
        guard next != current else { return }
        move(to: next) { [weak self] in self?.record(next) }
    }

    /// Load the page, *then* move to it.
    ///
    /// Nothing draws until there is something to draw. Arriving first and
    /// fetching afterwards means a frame of the word "Album" over an empty
    /// list, and then the real page flickering in underneath it — the same
    /// partial render a web page does, and it reads exactly as badly. These are
    /// indexed queries answered in-process; there is no reason to show anybody
    /// the gap.
    ///
    /// One task at a time, so a second click while the first page is still
    /// being read wins: the older move is cancelled before it can apply, rather
    /// than landing on top of the newer one.
    ///
    /// `arriving` is where the history moves, and it runs beside the page
    /// rather than before it. Back used to step the cursor on the click and
    /// leave the move to catch up, so a Back pressed while a record was still
    /// being read cancelled that record — which then never applied and never
    /// recorded — and stepped off a page nobody had arrived at. The screen did
    /// not move, and it took a second press to go anywhere. Nothing about where
    /// you are changes until there is a page to be there.
    private func move(to next: Page, arriving: @escaping @MainActor () -> Void) {
        moving?.cancel()
        moving = Task {
            await Trace.region("click-to-page") {
                let listing = await Trace.region("prepare") { await prepared(for: next) }
                guard !Task.isCancelled else { return }
                Trace.region("apply") {
                    apply(next, showing: listing)
                    arriving()
                }
            }
        }
    }

    /// Everything the page draws, in hand before it is shown. A listing when
    /// the page is a section that has rows of its own; the pages about one
    /// thing hold theirs on the model that read them.
    private func prepared(for page: Page) async -> LibraryModel.Listing? {
        switch page {
        case .album(let id):
            await library.prepare(album: id)
            return nil
        case .artist(let id):
            await library.prepare(artist: id)
            return nil
        case .section(.playlist(let id)):
            await playlists?.prepare(id: id)
            return await library.prepare(section: .playlist(id))
        case .section(let section):
            return await library.prepare(section: section)
        }
    }

    /// Record where we just went, the way a browser does.
    private func record(_ next: Page) {
        // The cursor can already point here: `forget` prunes history without
        // moving the screen, and what follows is usually a move back onto the
        // entry it left the cursor on.
        guard history[cursor] != next else { return }
        // A new move discards anything ahead.
        if cursor < history.count - 1 {
            history.removeSubrange((cursor + 1)...)
        }
        history.append(next)
        cursor = history.count - 1
    }

    /// Drop a section from history. Search results only exist while there is a
    /// query, and a deleted playlist no longer exists at all; once either is
    /// gone, they are not somewhere Back can return to.
    func forget(_ section: Section) {
        guard history.contains(.section(section)) else { return }
        // Whatever survives behind the cursor keeps its order, so the cursor
        // lands on the entry it was on — or on the last one, if that entry was
        // itself forgotten.
        let surviving = history[..<cursor].filter { $0 != .section(section) }.count
        history.removeAll { $0 == .section(section) }
        if history.isEmpty { history = [current] }
        cursor = min(surviving, history.count - 1)
    }

    /// The move itself: the page and the rows it draws, in one change. Two
    /// changes would be two renders, and the first of them would be the page
    /// without its rows.
    private func apply(_ next: Page, showing listing: LibraryModel.Listing?) {
        current = next
        // Only a section decides what the library shows; arriving at a record
        // is not a reason to throw away the filter behind it.
        if let listing { library.show(listing) }
    }

    // MARK: - Bindings

    /// What the sidebar highlights, and what clicking it means.
    ///
    /// A row is lit when the page *is* that row, so a record or an artist lights
    /// nothing. Nothing is derived from a path any more, so there is no write to
    /// guard against: a `List` rebuilding and writing its selection back is
    /// either the page it already shows, which `go(to:)` discards, or a click.
    var sidebarSelection: Binding<Section?> {
        Binding(
            get: { self.current.section },
            set: { [weak self] chosen in
                guard let self, let chosen else { return }
                show(chosen)
            }
        )
    }

    // MARK: - Playback

    /// Show the queue once it holds what was just started.
    ///
    /// Playing something is not a reason to move: you stay where you were, and
    /// the queue keeps running behind you until you go to it yourself. The one
    /// exception is the artist page — a wall of covers with no tracks on it,
    /// where nothing on screen would show that playback had started at all.
    ///
    /// Queue mutations run off the main actor, so switching immediately shows
    /// the old queue for a frame or two and then flickers. Waiting for the
    /// engine to confirm avoids that — but only briefly: if the mutation is
    /// slow enough to notice, jumping to the queue after the fact would feel
    /// like the app moving on its own, so it stays put instead.
    func showQueueWhenReady(watching player: PlayerModel) {
        let before = player.queueVersion
        Task {
            await player.settle(within: .milliseconds(50)) { player.queueVersion != before }
            if player.queueVersion != before { show(.queue) }
        }
    }
}
