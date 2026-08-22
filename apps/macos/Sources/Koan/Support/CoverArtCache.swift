import AppKit
import KoanFFI

/// Album art, keyed by album rather than by track — every track on a record
/// carries the same embedded image, so keying by track would decode the same
/// JPEG a dozen times per album.
///
/// Nothing is prefetched. A `LazyVGrid` only builds visible cells, so requests
/// arrive roughly a screenful at a time and stop when the user stops scrolling.
/// That matters more than it looks: for a remote-backed library each miss is an
/// HTTP round trip to the Subsonic server, not a tag read.
@MainActor
@Observable
final class CoverArtCache {
    /// `nil` value means "looked, nothing there" — distinct from "not looked yet",
    /// which keeps missing art from being retried on every redraw.
    private var albumArt: [Int64: NSImage?] = [:]
    private var trackArt: [Int64: NSImage?] = [:]
    private var inFlight: Set<Int64> = []

    private let engine: KoanEngine

    init(engine: KoanEngine) {
        self.engine = engine
    }

    /// Art for an album. Returns what's cached and schedules a load otherwise;
    /// the view re-renders when it arrives.
    func art(albumId: Int64) -> NSImage? {
        if let cached = albumArt[albumId] { return cached }
        guard !inFlight.contains(albumId) else { return nil }
        inFlight.insert(albumId)

        let engine = self.engine
        Task {
            let image = await Task.detached(priority: .utility) { () -> NSImage? in
                // Any track off the record will do — they share the artwork.
                guard let tracks = try? engine.tracks(
                    albumId: albumId, artistId: nil, sort: .album, limit: 1, offset: 0
                ) else { return nil }
                guard let first = tracks.first(where: { $0.path != nil }) ?? tracks.first,
                      let art = try? engine.coverArt(trackId: first.id, size: 400)
                else { return nil }
                return NSImage(data: Data(art.data))
            }.value

            albumArt[albumId] = image
            inFlight.remove(albumId)
        }
        return nil
    }

    /// Art for a specific track — used by the transport bar, where the album may
    /// not be known (queue items can outlive a library row).
    func art(trackId: Int64) -> NSImage? {
        if let cached = trackArt[trackId] { return cached }
        let key = -trackId  // separate keyspace from album ids
        guard !inFlight.contains(key) else { return nil }
        inFlight.insert(key)

        let engine = self.engine
        Task {
            let image = await Task.detached(priority: .utility) { () -> NSImage? in
                guard let art = try? engine.coverArt(trackId: trackId, size: 600) else { return nil }
                return NSImage(data: Data(art.data))
            }.value

            trackArt[trackId] = image
            inFlight.remove(key)
        }
        return nil
    }
}
