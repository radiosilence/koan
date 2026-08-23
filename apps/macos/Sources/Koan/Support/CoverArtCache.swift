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
    private(set) var inFlight: Set<String> = []

    /// Which albums produced each image, by content hash.
    ///
    /// Navidrome answers with a stock placeholder — a blue vinyl with its own
    /// logo on it — for anything with no artwork, and it looks like real art
    /// until you notice every artless record has the same one. There is no flag
    /// to ask about, but the placeholder gives itself away by being
    /// byte-identical across albums: the moment one image turns up for a second
    /// album, it isn't album art.
    private var hashOwners: [String: Set<String>] = [:]
    private var placeholderHashes: Set<String> = []

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

    /// Whether a fetch is outstanding — distinct from having looked and found
    /// nothing, which should show a placeholder rather than a spinner.
    func isLoading(albumId: Int64) -> Bool { inFlight.contains("album-\(albumId)") }
    func isLoading(trackId: Int64) -> Bool { inFlight.contains("track-\(trackId)") }

    // MARK: - Loading

    private func image(key: String, fetch: @escaping @Sendable (KoanEngine) -> Data?) -> NSImage? {
        if let cached = memory[key] { return cached }
        guard !inFlight.contains(key) else { return nil }
        inFlight.insert(key)

        let engine = self.engine
        let file = directory?.appendingPathComponent(Self.filename(for: key))
        Task {
            let data = await Task.detached(priority: .utility) { () -> Data? in
                if let file, let cached = try? Data(contentsOf: file) { return cached }
                guard let fetched = fetch(engine) else { return nil }
                if let file {
                    // Best effort: a cache that fails to write is still a cache.
                    try? fetched.write(to: file, options: .atomic)
                }
                return fetched
            }.value

            defer { inFlight.remove(key) }
            guard let data else {
                memory[key] = NSImage?.none
                return
            }
            memory[key] = accept(data, for: key)
        }
        return nil
    }

    /// Store the image unless it turns out to be the server's placeholder.
    private func accept(_ data: Data, for key: String) -> NSImage? {
        let hash = Self.digest(data)
        if placeholderHashes.contains(hash) { return nil }

        hashOwners[hash, default: []].insert(key)
        guard hashOwners[hash]?.count ?? 0 > 1 else {
            return NSImage(data: data)
        }

        // Second sighting: it's a placeholder. Forget everyone who got it.
        placeholderHashes.insert(hash)
        for owner in hashOwners[hash] ?? [] {
            memory[owner] = NSImage?.none
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

    private static func digest(_ data: Data) -> String {
        SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
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
