import Foundation
import KoanFFI
import SwiftUI

/// The playlists, and everything you can do to them.
///
/// Kept whole in memory: a playlist row is a name and two counts, and there are
/// tens of them, not thousands. Their *contents* are not — those are loaded a
/// playlist at a time, the same way an album's tracks are.
///
/// Every mutation writes through the engine and then reloads the list. The
/// engine pushes to the server on its own, in the background; nothing here
/// waits on a network.
@MainActor
@Observable
final class PlaylistsModel {
    let engine: KoanEngine

    private(set) var playlists: [Playlist] = []

    /// The playlist on screen, and its entries. Only one is ever open.
    ///
    /// Entries, not tracks: a playlist may hold the same track twice, so a
    /// track id names neither a row nor a place. The entry id does.
    private(set) var openId: Int64?
    private(set) var entries: [PlaylistEntry] = []
    private(set) var isLoading = false

    /// Up to four cover sources per playlist, for its 2×2 tile. Loaded lazily
    /// and held, because the sidebar draws every one of them on every update
    /// and re-asking the database for them each time would be a query per row
    /// per keystroke.
    private(set) var covers: [Int64: [AlbumArtwork.Source]] = [:]

    /// The `changedAt` each playlist's mosaic was built from. Adding tracks to
    /// a playlist changes its face, and without this the tile kept whatever it
    /// had the first time it was drawn — for a new playlist, nothing at all.
    private var coverStamp: [Int64: String] = [:]

    /// Tracks waiting for a name, and the new playlist they will become.
    ///
    /// A request rather than a dialog, because the dialog cannot live where it
    /// is asked for: a context menu is gone the instant you pick from it, and
    /// takes any alert attached to it with it — which is why **Add to
    /// Playlist → New Playlist…** did nothing at all. One host presents this,
    /// somewhere that outlives the gesture. Empty is a real request: it means a
    /// new playlist with nothing in it yet.
    var naming: [Int64]?

    /// Set by `AppState`, so a slow reload shows up alongside everything else.
    weak var activity: ActivityModel?
    /// Somewhere to report a failure the user should see. Set by `AppState`.
    var report: ((String) -> Void)?

    init(engine: KoanEngine) {
        self.engine = engine
    }

    func playlist(id: Int64) -> Playlist? {
        playlists.first { $0.id == id }
    }

    // MARK: - Loading

    func load() {
        let engine = self.engine
        Task {
            playlists = (try? await engine.playlists()) ?? []
            // A playlist whose contents changed has a different mosaic, and a
            // deleted one should stop holding memory.
            let live = Set(playlists.map(\.id))
            covers = covers.filter { live.contains($0.key) }
            coverStamp = coverStamp.filter { live.contains($0.key) }
            for playlist in playlists where coverStamp[playlist.id] != playlist.changedAt {
                await loadCovers(for: playlist.id)
                coverStamp[playlist.id] = playlist.changedAt
            }
        }
    }

    /// Open a playlist and load its rows. Asking for the one already open is
    /// how a library change reaches this page — the rows are re-read without
    /// blanking first, so nothing flashes empty for the length of a query.
    func open(id: Int64) {
        if openId != id {
            openId = id
            entries = []
        }
        reloadTracks()
    }

    func reloadTracks() {
        guard let id = openId else { return }
        let engine = self.engine
        isLoading = true
        Task {
            let rows = (try? await engine.playlistTracks(playlistId: id)) ?? []
            // The page moved on while we were reading.
            guard openId == id else { return }
            entries = rows
            isLoading = false
        }
    }

    private func loadCovers(for id: Int64) async {
        let ids = (try? await engine.playlistCoverAlbumIds(playlistId: id)) ?? []
        covers[id] = ids.map { .album($0) }
    }

    // MARK: - Mutations

    /// Create a playlist, optionally with tracks already in it. Hands back the
    /// new playlist so the caller can navigate to it.
    @discardableResult
    func create(named name: String, trackIds: [Int64] = []) async -> Playlist? {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        do {
            return try await engine.createPlaylist(name: trimmed, trackIds: trackIds)
        } catch {
            report?("Couldn't create that playlist — \(error.localizedDescription)")
            return nil
        }
    }

    /// Ask for a name for a new playlist holding whatever was dropped.
    ///
    /// Resolving happens here rather than in the drop handler because a drop on
    /// a sidebar row is over by the time the database answers, and an artist is
    /// thousands of rows.
    func beginNaming(dropped: [PlayableTransfer]) {
        Task { naming = await resolve(dropped) }
    }

    func rename(id: Int64, to name: String) {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        act { try await $0.renamePlaylist(playlistId: id, name: trimmed) }
    }

    func delete(id: Int64) {
        if openId == id {
            openId = nil
            entries = []
        }
        act { _ = try await $0.deletePlaylist(playlistId: id) }
    }

    func add(trackIds: [Int64], to id: Int64) {
        guard !trackIds.isEmpty else { return }
        act {
            _ = try await $0.addToPlaylist(playlistId: id, trackIds: trackIds)
        }
    }

    /// Resolve dropped playables and append them. Order is preserved, so
    /// dropping three albums at once adds them in the order they were dragged.
    func add(dropped: [PlayableTransfer], to id: Int64) {
        Task {
            let ids = await resolve(dropped)
            guard !ids.isEmpty else { return }
            add(trackIds: ids, to: id)
        }
    }

    /// The same, landing where they were dropped rather than at the end. The
    /// engine adds and reorders in one go — doing it in two from here would be
    /// reordering against the list as it was before the add landed.
    func insert(dropped: [PlayableTransfer], into id: Int64, at position: Int) {
        Task {
            let ids = await resolve(dropped)
            guard !ids.isEmpty else { return }
            act {
                _ = try await $0.insertIntoPlaylist(
                    playlistId: id, trackIds: ids, at: UInt32(position)
                )
            }
        }
    }

    /// Put the entries in this order. Ids survive, so the queue keeps knowing
    /// which row each of its items came from.
    func reorder(entryIds: [Int64], in id: Int64) {
        act {
            try await $0.reorderPlaylist(playlistId: id, entryIds: entryIds)
        }
    }

    func remove(entryIds: [Int64], from id: Int64) {
        guard !entryIds.isEmpty else { return }
        act {
            _ = try await $0.removeFromPlaylist(playlistId: id, entryIds: entryIds)
        }
    }

    /// Shuffle the playlist itself, permanently. Distinct from playing it
    /// shuffled, which leaves it alone.
    func shuffle(id: Int64) {
        act { try await $0.shufflePlaylist(playlistId: id) }
    }

    /// Put `moving` where `target` currently sits.
    ///
    /// Applied locally first, so the row lands under the pointer rather than a
    /// round trip later. Dropping a playlist onto itself is a no-op rather than
    /// an error — it is the commonest way to abandon a drag.
    func reorder(moving: [Int64], onto target: Int64) {
        let doomed = Set(moving)
        guard !doomed.contains(target) else { return }
        var order = playlists.map(\.id).filter { !doomed.contains($0) }
        guard let at = order.firstIndex(of: target) else { return }
        order.insert(contentsOf: moving, at: at)

        let byId = Dictionary(playlists.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
        playlists = order.compactMap { byId[$0] }
        let engine = self.engine
        Task { try? await engine.reorderPlaylists(ids: order) }
    }

    /// Remember how this playlist is looked at. Local to this machine — a
    /// server has nowhere to put it.
    func setGrouped(_ grouped: Bool, for id: Int64) {
        if let index = playlists.firstIndex(where: { $0.id == id }) {
            playlists[index].grouped = grouped
        }
        let engine = self.engine
        Task { try? await engine.setPlaylistGrouped(playlistId: id, grouped: grouped) }
    }

    /// Write it out as an M3U8. Reports what could not go in: a playlist file
    /// is a list of paths, and a remote track that has never been downloaded
    /// has none.
    func export(id: Int64, to url: URL) {
        let engine = self.engine
        Task {
            do {
                let summary = try await engine.exportPlaylist(
                    playlistId: id, destPath: url.path
                )
                if summary.skipped > 0 {
                    report?(
                        "Exported \(Format.count(Int64(summary.written), "track")) — "
                            + "\(summary.skipped) aren't downloaded, so they have no file to point at."
                    )
                }
            } catch {
                report?("Couldn't write that playlist — \(error.localizedDescription)")
            }
        }
    }

    // MARK: - Helpers

    private func resolve(_ dropped: [PlayableTransfer]) async -> [Int64] {
        var ids: [Int64] = []
        for item in dropped {
            ids += await item.trackIds(using: engine)
        }
        return ids
    }

    /// A mutation, with the error reporting every one of them wants.
    ///
    /// Nothing reloads here. Writing playlist rows is a library change like any
    /// other, so the engine says so and whatever is on screen asks again — the
    /// same path a sync from the server takes. A reload written in per mutation
    /// is one that only covers the mutations somebody remembered.
    private func act(_ body: @escaping (KoanEngine) async throws -> Void) {
        let engine = self.engine
        Task {
            do {
                try await body(engine)
            } catch {
                report?(String(describing: error))
            }
        }
    }
}
