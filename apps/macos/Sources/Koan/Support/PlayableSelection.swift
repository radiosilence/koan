#if canImport(AppKit)
import AppKit
#endif
import KoanFFI
import Observation

/// Things picked out of a page to play or queue together — records in a grid,
/// or artists, records and tracks in search results.
///
/// A mode rather than a click: a click on a tile already plays the record, and
/// the title opens it. While selecting, a click ticks instead — ⇧ for a range,
/// ⌘A for everything the page is showing — and playing or queueing the pick
/// puts the page back to normal.
///
/// Held in the order things were ticked, not the order of the page. A pick can
/// span several filters or queries — narrow, tick, narrow again, tick more —
/// and the page has no order for something it is no longer showing.
///
/// One per page: a pick belongs to the page it was made in, and ends when that
/// page leaves the screen.
@MainActor
@Observable
final class PlayableSelection {
    /// What the page is showing, in its order — where ⇧ and ⌘A look.
    @ObservationIgnored let grid: @MainActor () -> [Playable]

    init(grid: @escaping @MainActor () -> [Playable]) {
        self.grid = grid
    }

    private(set) var isActive = false
    private(set) var picked: [Playable] = []

    /// Where a ⇧-click range starts: the last item clicked without ⇧.
    @ObservationIgnored private var anchor: Playable.Key?
    /// Membership for the checkmarks, so a tick is not a walk of the list per
    /// item.
    private var members: Set<Playable.Key> = []

    func contains(_ key: Playable.Key) -> Bool { members.contains(key) }

    func begin(with playable: Playable? = nil) {
        isActive = true
        if let playable { toggle(playable) }
    }

    func end() {
        guard isActive else { return }
        isActive = false
        picked = []
        members = []
        anchor = nil
    }

    /// A click on an item: ⇧ extends from the last one clicked, anything else
    /// flips just this one. A tap has no ⇧, so it always flips.
    func click(_ playable: Playable) {
        #if canImport(AppKit)
        if NSEvent.modifierFlags.contains(.shift), let anchor {
            extend(from: anchor, to: playable, in: grid())
            return
        }
        #endif
        toggle(playable)
    }

    /// A click on something that does something else outside the mode — goes
    /// somewhere, plays. Whether the pick took it: a tick while selecting, or
    /// a new selection with ⌘ held.
    func take(_ playable: Playable) -> Bool {
        if isActive {
            click(playable)
            return true
        }
        #if canImport(AppKit)
        if NSEvent.modifierFlags.contains(.command) {
            begin(with: playable)
            return true
        }
        #endif
        return false
    }

    func selectAll() {
        isActive = true
        add(grid())
    }

    private func toggle(_ playable: Playable) {
        let key = playable.key
        anchor = key
        if members.remove(key) != nil {
            picked.removeAll { $0.key == key }
        } else {
            members.insert(key)
            picked.append(playable)
        }
    }

    /// Everything between the two, in page order — the way a list extends.
    private func extend(from anchor: Playable.Key, to playable: Playable, in grid: [Playable]) {
        guard let start = grid.firstIndex(where: { $0.key == anchor }),
              let end = grid.firstIndex(where: { $0.key == playable.key })
        else { return toggle(playable) }
        let range = start <= end ? start...end : end...start
        add(Array(grid[range]))
    }

    private func add(_ items: [Playable]) {
        picked += items.filter { members.insert($0.key).inserted }
    }

    /// Play or queue the pick, and put the page back.
    func commit(engine: KoanEngine, player: PlayerModel, play: Bool) {
        let items = picked
        end()
        Task {
            var tracks: [Int64] = []
            for item in items {
                tracks += await item.trackIds(using: engine)
            }
            if play { player.playNow(trackIds: tracks) } else { player.enqueue(trackIds: tracks) }
        }
    }
}
