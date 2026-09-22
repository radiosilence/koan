import AppKit
import KoanFFI
import Observation

/// Albums picked out of the grid to play or queue together.
///
/// A mode rather than a click: a click on a tile already plays the record, and
/// the title opens it. While selecting, a click ticks instead — ⇧ for a range,
/// ⌘A for everything the filter is showing — and playing or queueing the pick
/// puts the grid back to normal.
///
/// Held in the order things were ticked, not the order of the grid. A pick can
/// span several filters — narrow, tick, narrow again, tick more — and the grid
/// has no order for a record it is no longer showing.
@MainActor
@Observable
final class AlbumSelection {
    private(set) var isActive = false
    private(set) var ids: [Int64] = []

    /// Where a ⇧-click range starts: the last tile clicked without ⇧.
    @ObservationIgnored private var anchor: Int64?
    /// Membership for the checkmarks, so a tick is not a walk of the list per
    /// tile.
    private var members: Set<Int64> = []

    func contains(_ id: Int64) -> Bool { members.contains(id) }

    func begin(with id: Int64? = nil) {
        isActive = true
        if let id { toggle(id) }
    }

    func end() {
        guard isActive else { return }
        isActive = false
        ids = []
        members = []
        anchor = nil
    }

    /// A click on a tile: ⇧ extends from the last one clicked, anything else
    /// flips just this one.
    func click(_ id: Int64, in grid: [Album]) {
        if NSEvent.modifierFlags.contains(.shift), let anchor {
            extend(from: anchor, to: id, in: grid)
        } else {
            toggle(id)
        }
    }

    func selectAll(_ grid: [Album]) {
        isActive = true
        add(grid.map(\.id))
    }

    private func toggle(_ id: Int64) {
        anchor = id
        if members.remove(id) != nil {
            ids.removeAll { $0 == id }
        } else {
            members.insert(id)
            ids.append(id)
        }
    }

    /// Everything between the two, in grid order — the way a list extends.
    private func extend(from anchor: Int64, to id: Int64, in grid: [Album]) {
        guard let start = grid.firstIndex(where: { $0.id == anchor }),
              let end = grid.firstIndex(where: { $0.id == id })
        else { return toggle(id) }
        let range = start <= end ? start...end : end...start
        add(grid[range].map(\.id))
    }

    private func add(_ picked: [Int64]) {
        let new = picked.filter { members.insert($0).inserted }
        if !new.isEmpty { ids += new }
    }

    /// Play or queue the pick, and put the grid back.
    func commit(engine: KoanEngine, player: PlayerModel, play: Bool) {
        let albums = ids
        end()
        Task {
            var tracks: [Int64] = []
            for album in albums {
                tracks += (try? await engine.trackIds(albumId: album, artistId: nil)) ?? []
            }
            if play { player.playNow(trackIds: tracks) } else { player.enqueue(trackIds: tracks) }
        }
    }
}
