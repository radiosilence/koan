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

    private let library: LibraryModel

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

    /// Load the record, *then* move to it.
    ///
    /// Nothing draws until there is something to draw. Arriving first and
    /// fetching afterwards means a frame of the word "Album" over an empty
    /// list, and then the real page flickering in underneath it — which is the
    /// same partial render a web page does and reads exactly as badly. The read
    /// is two indexed queries; there is no reason to show anybody the gap.
    func open(album id: Int64, highlighting trackId: Int64? = nil) {
        FrameTimer.shared.begin()
        Task {
            await Trace.region("click-to-album") {
                await Trace.region("prepare") { await library.prepare(album: id) }
                highlightedTrackId = trackId
                Trace.region("apply") { go(to: .album(id)) }
            }
        }
    }

    func open(artist id: Int64) {
        go(to: .artist(id))
    }

    func open(playlist id: Int64) {
        go(to: .section(.playlist(id)))
    }

    func goBack() {
        guard canGoBack else { return }
        cursor -= 1
        step(to: history[cursor])
    }

    func goForward() {
        guard canGoForward else { return }
        cursor += 1
        step(to: history[cursor])
    }

    /// Back and forward land on record pages too, and they want what `open`
    /// wants: the page ready before it is shown.
    private func step(to page: Page) {
        guard case .album(let id) = page else { return apply(page) }
        Task {
            await library.prepare(album: id)
            apply(page)
        }
    }

    /// Go to a page, recording it. The only way anything moves.
    func go(to next: Page) {
        guard next != current else { return }
        apply(next)
        // The cursor can already point here: `forget` prunes history without
        // moving the screen, and what follows is usually a move back onto the
        // entry it left the cursor on.
        guard history[cursor] != next else { return }
        // A new move discards anything ahead, the way a browser does.
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

    private func apply(_ next: Page) {
        current = next
        // Only a section decides what the library loads; arriving at a record
        // is not a reason to throw away the filter behind it.
        if let section = next.section, section != library.section {
            library.showing(section)
        }
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
            let deadline = ContinuousClock.now + .milliseconds(50)
            while ContinuousClock.now < deadline {
                if player.queueVersion != before {
                    show(.queue)
                    return
                }
                try? await Task.sleep(for: .milliseconds(5))
            }
        }
    }
}
