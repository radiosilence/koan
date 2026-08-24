import SwiftUI

/// A pushed destination.
///
/// One enum rather than a type per kind. A `NavigationStack` matches
/// destinations by type, so two `Int64`-shaped routes in one stack collide
/// silently and send you to whichever was registered first. It also makes the
/// stack a plain `Equatable` array, which is what lets the navigator tell a
/// real move from SwiftUI echoing back what is already there.
enum Route: Hashable {
    case album(Int64)
    case artist(Int64)

    /// The section this thing lives in, for jumps that arrive from outside any
    /// section — search, mainly.
    var home: Navigator.Section {
        switch self {
        case .album: .albums
        case .artist: .artists
        }
    }
}

/// Where the app is, as one value.
///
/// The section and the stack pushed on top of it move together or not at all.
/// Splitting them is what made navigation unpredictable: a push could be undone
/// by an unrelated update that only meant to change the section, and the
/// symptom was always the same and always misleading — the click registered and
/// nothing happened.
struct Location: Hashable {
    var section: Navigator.Section
    var stack: [Route] = []
}

/// The one owner of where you are.
///
/// Nothing else writes the detail stack's path, and nothing derives state from
/// it that can be written back. Every move — a section, a push, a pop, a
/// history replay — goes through `go(to:)`, which is also the only place that
/// records history, so back and forward can never disagree with the screen.
///
/// The library follows the location rather than the other way round: what is
/// loaded is a consequence of where you are.
@MainActor
@Observable
final class Navigator {
    enum Section: Hashable, Identifiable {
        case queue
        case searchResults
        case albums
        case artists
        case favourites
        case playHistory
        case snapshots

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
            case .queue, .searchResults, .snapshots: nil
            }
        }
    }

    private(set) var location = Location(section: .queue)

    /// The track a jump was aimed at, so the album view can single it out
    /// rather than dropping you at the top of a twenty-track record. Cleared by
    /// the view once it has scrolled to it.
    var highlightedTrackId: Int64?

    /// Every location actually reached. A `NavigationStack` only goes back
    /// within one stack, so it cannot return you across a section switch or a
    /// jump from search — which is what "back" means to someone using the app.
    private var history: [Location] = [Location(section: .queue)]
    private var cursor = 0

    private let library: LibraryModel

    init(library: LibraryModel) {
        self.library = library
    }

    var section: Section { location.section }
    var canGoBack: Bool { cursor > 0 }
    var canGoForward: Bool { cursor < history.count - 1 }

    // MARK: - Moves

    /// A section, at its root. Asking for the section you are already in comes
    /// back out of whatever you pushed onto it, which is what ⌘2 on an album
    /// page should do.
    func show(_ section: Section) {
        go(to: Location(section: section))
    }

    /// Push onto whatever is showing, so Back returns you to the door you came
    /// through rather than to a list you never visited.
    func open(album id: Int64, highlighting trackId: Int64? = nil) {
        highlightedTrackId = trackId
        push(.album(id))
    }

    func open(artist id: Int64) {
        push(.artist(id))
    }

    /// Land in the section a thing lives in, with the thing pushed.
    ///
    /// For arrivals from search: the results page exists only while there is a
    /// query, so pushing onto it would leave you standing on a root that is
    /// about to empty. Back returns to what you were doing before you searched.
    func jump(to route: Route, highlighting trackId: Int64? = nil) {
        highlightedTrackId = trackId
        go(to: Location(section: route.home, stack: [route]))
    }

    func goBack() {
        guard canGoBack else { return }
        cursor -= 1
        apply(history[cursor])
    }

    func goForward() {
        guard canGoForward else { return }
        cursor += 1
        apply(history[cursor])
    }

    /// Go somewhere recorded earlier, stack and all.
    func go(to next: Location) {
        guard next != location else { return }
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
    /// query; once it is gone, they are not somewhere Back can return to.
    func forget(_ section: Section) {
        guard history.contains(where: { $0.section == section }) else { return }
        // Whatever survives behind the cursor keeps its order, so the cursor
        // lands on the entry it was on — or on the last one, if that entry was
        // itself forgotten.
        let surviving = history[..<cursor].filter { $0.section != section }.count
        history.removeAll { $0.section == section }
        if history.isEmpty { history = [location] }
        cursor = min(surviving, history.count - 1)
    }

    private func push(_ route: Route) {
        var next = location
        next.stack.append(route)
        go(to: next)
    }

    private func apply(_ next: Location) {
        let changedSection = next.section != location.section
        location = next
        if changedSection { library.showing(next.section) }
    }

    // MARK: - Bindings

    /// The detail stack's path.
    ///
    /// SwiftUI writes back through this whenever the stack believes its
    /// contents changed, including writes carrying what is already there.
    /// Comparing first is what makes those harmless — only a real change moves
    /// anything, and a real change is a move like any other.
    var stack: Binding<[Route]> {
        Binding(
            get: { self.location.stack },
            set: { [weak self] written in
                guard let self, written != location.stack else { return }
                var next = location
                next.stack = written
                go(to: next)
            }
        )
    }

    /// What the sidebar highlights, and what clicking it means.
    ///
    /// Stored, never derived from the stack: deriving the highlight from the
    /// path turned every push into a write, and the write popped the thing just
    /// pushed. A write carrying the section already showing is discarded for
    /// the same reason — a `List` writes back whenever it decides its selection
    /// moved, which includes rebuilds it does for its own reasons, and the
    /// Results row appearing or vanishing is one of those.
    ///
    /// So clicking the lit row does nothing. Back is the way out of a detail
    /// view, from the toolbar or ⌘[, wherever you reached it from.
    var sidebarSelection: Binding<Section?> {
        Binding(
            get: { self.location.section },
            set: { [weak self] chosen in
                guard let self, let chosen, chosen != location.section else { return }
                show(chosen)
            }
        )
    }

    // MARK: - Playback

    /// Show the queue once it holds what was just started.
    ///
    /// Called after anything that starts playback outright, so you end up
    /// looking at what you started. Not called for "add to queue" or "play
    /// next" — those are things you do while browsing, and being thrown across
    /// the app for them would be rude.
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
