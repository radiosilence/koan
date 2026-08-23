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
    /// One fetch per key, shared by everyone who asks while it is running.
    private var tasks: [String: Task<NSImage?, Never>] = [:]

    /// Which albums produced each image, by content hash.
    ///
    /// Navidrome answers with a stock placeholder — a blue vinyl with its own
    /// logo on it — for anything with no artwork, and it looks like real art
    /// until you notice every artless record has the same one. There is no flag
    /// to ask about, but the placeholder gives itself away by repeating across
    /// unrelated albums.
    ///
    /// Only *album* lookups teach this, and it takes three albums to conclude
    /// it. Learning from track lookups was wrong in a way that destroyed real
    /// art: a track's cover is by definition the same image as its album's, so
    /// playing an album made its own artwork look like a repeat and wiped it
    /// from the grid. Two albums sharing art is also legitimate — a single and
    /// the record it came from — so two sightings is too eager.
    private var hashOwners: [String: Set<String>] = [:]
    private var placeholderHashes: Set<String> = []

    private let engine: KoanEngine
    private let directory: URL?

    init(engine: KoanEngine) {
        self.engine = engine
        self.directory = Self.makeDirectory()
    }

    // MARK: - Lookup

    /// What's already in memory, without starting anything.
    ///
    /// Views call this first so a cover that has been seen appears in the same
    /// frame rather than after an await, which is what made a scrolled-back grid
    /// flash grey.
    func cached(_ source: AlbumArtwork.Source) -> NSImage? {
        memory[Self.key(source)] ?? nil
    }

    /// Fetch the art, awaiting the disk cache or the network as needed.
    ///
    /// Deliberately `async` and not observable: an earlier version returned
    /// what it had and started a load as a side effect of being read, so every
    /// artwork on screen depended on every entry in the cache and one arriving
    /// invalidated all of them.
    ///
    /// Concurrent callers for the same key share one fetch rather than racing —
    /// a grid scrolling past forty covers of the same record should be one
    /// round trip.
    func image(for source: AlbumArtwork.Source) async -> NSImage? {
        let key = Self.key(source)
        if let cached = memory[key] { return cached }
        if let running = tasks[key] { return await running.value }

        let engine = self.engine
        let file = directory?.appendingPathComponent(Self.filename(for: key))
        let task = Task<NSImage?, Never> { [weak self] in
            let data = await Task.detached(priority: .utility) { () -> Data? in
                if let file, let cached = try? Data(contentsOf: file) { return cached }
                guard let fetched = Self.fetch(source, engine) else { return nil }
                if let file {
                    // Best effort: a cache that fails to write is still a cache.
                    try? fetched.write(to: file, options: .atomic)
                }
                return fetched
            }.value

            guard let self else { return nil }
            self.tasks[key] = nil
            guard let data else {
                self.memory[key] = NSImage?.none
                return nil
            }
            let image = self.accept(data, for: key)
            self.memory[key] = image
            return image
        }
        tasks[key] = task
        return await task.value
    }

    private static func key(_ source: AlbumArtwork.Source) -> String {
        switch source {
        case .album(let id): "album-\(id)"
        case .track(let id): "track-\(id)"
        }
    }

    private nonisolated static func fetch(
        _ source: AlbumArtwork.Source,
        _ engine: KoanEngine
    ) -> Data? {
        switch source {
        case .album(let albumId):
            // Any track off the record will do — they share the artwork.
            guard let tracks = try? engine.tracks(
                albumId: albumId, artistId: nil, sort: .album, limit: 1, offset: 0
            ) else { return nil }
            guard let first = tracks.first(where: { $0.path != nil }) ?? tracks.first else {
                return nil
            }
            return (try? engine.coverArt(trackId: first.id, size: 400))?.data
        case .track(let trackId):
            return (try? engine.coverArt(trackId: trackId, size: 600))?.data
        }
    }

    /// Store the image unless it turns out to be the server's placeholder.
    private func accept(_ data: Data, for key: String) -> NSImage? {
        let hash = Self.digest(data)
        if placeholderHashes.contains(hash) { return nil }

        // Track lookups never teach: a track's cover is its album's cover, so
        // counting it would make every played album look like a repeat.
        guard key.hasPrefix("album-") else { return NSImage(data: data) }

        hashOwners[hash, default: []].insert(key)
        guard hashOwners[hash]?.count ?? 0 >= 3 else {
            return NSImage(data: data)
        }

        // Three unrelated albums with byte-identical art is a placeholder.
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
