import AppKit
import CryptoKit
import KoanFFI

/// Album art, cached in memory and on disk.
///
/// Keyed by album rather than by track — every track on a record carries the
/// same image, so keying by track would decode the same JPEG a dozen times per
/// album.
///
/// The disk half matters more than it looks. On a remote-backed library every
/// miss is an HTTP round trip to the Subsonic server, so without it the same
/// artwork is re-fetched on every launch and every scroll back up the grid.
/// Files live under `~/Library/Caches`, which is the right place for data the
/// system may reclaim and we can always fetch again.
@MainActor
@Observable
final class CoverArtCache {
    /// `nil` value means "looked, nothing there" — distinct from "not looked
    /// yet", which keeps missing art from being retried on every redraw.
    private var memory: [String: NSImage?] = [:]
    private var inFlight: Set<String> = []

    private let engine: KoanEngine
    private let directory: URL?

    init(engine: KoanEngine) {
        self.engine = engine
        self.directory = Self.makeDirectory()
    }

    // MARK: - Lookup

    /// Art for an album. Returns what's cached and schedules a load otherwise;
    /// the view re-renders when it arrives.
    func art(albumId: Int64) -> NSImage? {
        image(key: "album-\(albumId)") { engine in
            // Any track off the record will do — they share the artwork.
            guard let tracks = try? engine.tracks(
                albumId: albumId, artistId: nil, sort: .album, limit: 1, offset: 0
            ) else { return nil }
            guard let first = tracks.first(where: { $0.path != nil }) ?? tracks.first else {
                return nil
            }
            return (try? engine.coverArt(trackId: first.id, size: 400))?.data
        }
    }

    /// Art for a specific track — used by the transport bar, where the album
    /// may not be known (queue items can outlive a library row).
    func art(trackId: Int64) -> NSImage? {
        image(key: "track-\(trackId)") { engine in
            (try? engine.coverArt(trackId: trackId, size: 600))?.data
        }
    }

    // MARK: - Loading

    private func image(key: String, fetch: @escaping @Sendable (KoanEngine) -> Data?) -> NSImage? {
        if let cached = memory[key] { return cached }
        guard !inFlight.contains(key) else { return nil }
        inFlight.insert(key)

        let engine = self.engine
        let file = directory?.appendingPathComponent(Self.filename(for: key))
        Task {
            let image = await Task.detached(priority: .utility) { () -> NSImage? in
                if let file, let data = try? Data(contentsOf: file) {
                    return NSImage(data: data)
                }
                guard let data = fetch(engine) else { return nil }
                if let file {
                    // Best effort: a cache that fails to write is still a cache.
                    try? data.write(to: file, options: .atomic)
                }
                return NSImage(data: data)
            }.value

            memory[key] = image
            inFlight.remove(key)
        }
        return nil
    }

    // MARK: - Disk

    private static func makeDirectory() -> URL? {
        guard let caches = FileManager.default.urls(
            for: .cachesDirectory, in: .userDomainMask
        ).first else { return nil }
        let dir = caches.appendingPathComponent("cc.blit.koan/artwork", isDirectory: true)
        do {
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            return dir
        } catch {
            return nil
        }
    }

    /// Hashed so the filename can't collide with anything or exceed a path
    /// limit, and carries no metadata of its own.
    private static func filename(for key: String) -> String {
        let digest = SHA256.hash(data: Data(key.utf8))
        return digest.map { String(format: "%02x", $0) }.joined()
    }

    /// Drop everything. Exposed for settings; the system may also reclaim the
    /// directory on its own.
    func purge() {
        memory.removeAll()
        guard let directory else { return }
        try? FileManager.default.removeItem(at: directory)
        _ = Self.makeDirectory()
    }

    var diskUsage: Int64 {
        guard let directory,
              let files = try? FileManager.default.contentsOfDirectory(
                  at: directory, includingPropertiesForKeys: [.fileSizeKey]
              )
        else { return 0 }
        return files.reduce(0) { total, url in
            total + Int64((try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0)
        }
    }
}
