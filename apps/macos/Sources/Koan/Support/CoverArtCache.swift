import AppKit
import CryptoKit
import ImageIO
import KoanFFI

/// Album art: bytes cached per record, bitmaps cached per record *and* size.
///
/// The two layers are keyed differently on purpose.
///
/// Bytes are fetched and stored once per record. On a remote-backed library
/// every miss is an HTTP round trip, and keying them per track fetched the same
/// sleeve once for every track on the record — a dozen requests and a dozen
/// identical files for one image. Files live under `~/Library/Caches`, which is
/// the right place for data the system may reclaim and we can always fetch
/// again.
///
/// Bitmaps are kept per `(record, size)`. A 28pt queue row and a 200pt grid
/// tile want very different bitmaps, and keeping the large one for both is how
/// an hour of browsing reached a gigabyte of decoded artwork that nothing ever
/// released. `NSCache` bounds it and hands memory back under pressure.
///
/// Full size is deliberately never cached. It is for one sheet at a time, and
/// decoding it from a warm disk cache is quick enough that holding a 1000px
/// bitmap for every record you happened to click on is not worth the resident
/// memory.
@MainActor
@Observable
final class CoverArtCache {
    /// What we ask the server for, and what lands on disk.
    ///
    /// One size serves the grid, the rows and the viewer, because there is one
    /// blob per record. Large enough that the opened sleeve holds up on a
    /// retina display, small enough that a library's worth of them is a cache
    /// rather than a second copy of the artwork.
    private static let sourcePixels: UInt32 = 1000

    /// Roughly 250 grid tiles, or thousands of row thumbnails. `NSCache` evicts
    /// under memory pressure on its own, so this is a ceiling rather than a
    /// target.
    private static let memoryBudget = 256 * 1024 * 1024

    private let memory: NSCache<NSString, NSImage> = {
        let cache = NSCache<NSString, NSImage>()
        cache.totalCostLimit = CoverArtCache.memoryBudget
        return cache
    }()

    /// Records we looked at and found nothing for — distinct from "not looked
    /// yet", which is what keeps missing art from being retried on every
    /// redraw. Checked ahead of `memory`, so a record turning out to be the
    /// server's placeholder invalidates every size of it at once without having
    /// to enumerate them.
    private var absent: Set<String> = []

    /// One byte fetch per record, shared by every size that wants it.
    private var loads: [String: Task<Fetched, Never>] = [:]
    /// One decode per record and size, shared by everyone on screen asking.
    private var decodes: [String: Task<NSImage?, Never>] = [:]

    /// Which records produced each image, by content hash.
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

    /// What's already decoded, without starting anything.
    ///
    /// Views call this first so a cover that has been seen appears in the same
    /// frame rather than after an await, which is what made a scrolled-back grid
    /// flash grey.
    func cached(_ source: AlbumArtwork.Source, size: AlbumArtwork.Size) -> NSImage? {
        guard !absent.contains(Self.key(source)) else { return nil }
        return memory.object(forKey: Self.key(source, size) as NSString)
    }

    /// Fetch the art at a given size, awaiting the disk cache or the network as
    /// needed.
    ///
    /// Deliberately `async` and not observable: an earlier version returned
    /// what it had and started a load as a side effect of being read, so every
    /// artwork on screen depended on every entry in the cache and one arriving
    /// invalidated all of them.
    ///
    /// Concurrent callers share work at both layers — a grid and a transport
    /// bar showing the same record at different sizes is one round trip and two
    /// decodes, and forty tiles of the same record is one of each.
    func image(for source: AlbumArtwork.Source, size: AlbumArtwork.Size) async -> NSImage? {
        if let ready = cached(source, size: size) { return ready }
        let key = Self.key(source, size)
        if let running = decodes[key] { return await running.value }

        let task = Task<NSImage?, Never> { [weak self] in
            guard let self else { return nil }
            let data = await self.bytes(for: source)
            self.decodes[key] = nil
            guard let data else { return nil }

            let image = await Task.detached(priority: .utility) {
                Self.decode(data, to: size)
            }.value
            guard let image else { return nil }
            // Full size belongs to whichever sheet asked for it and goes when
            // that sheet does.
            if size != .full {
                self.memory.setObject(image, forKey: key as NSString, cost: Self.cost(image))
            }
            return image
        }
        decodes[key] = task
        return await task.value
    }

    // MARK: - Bytes

    /// The record's encoded artwork, from disk or the network. One fetch per
    /// record however many sizes are waiting on it.
    private func bytes(for source: AlbumArtwork.Source) async -> Data? {
        let key = Self.key(source)
        guard !absent.contains(key) else { return nil }

        let payload: Fetched
        if let running = loads[key] {
            payload = await running.value
        } else {
            let engine = self.engine
            let file = directory?.appendingPathComponent(Self.filename(for: key))
            let task = Task<Fetched, Never> {
                // Detached for the disk cache, not for the engine: reading and
                // writing the file would otherwise happen on the main actor. The
                // hash comes back with the bytes because it is a pass over the
                // whole payload and the main actor has no business doing it.
                await Task.detached(priority: .utility) { () -> Fetched in
                    if let file, let cached = try? Data(contentsOf: file) {
                        return .art(cached, hash: Self.digest(cached))
                    }
                    let fetched = await Self.fetch(source, engine)
                    if case .art(let bytes, _) = fetched, let file {
                        // Best effort: a cache that fails to write is still a cache.
                        try? bytes.write(to: file, options: .atomic)
                    }
                    return fetched
                }.value
            }
            loads[key] = task
            payload = await task.value
            loads[key] = nil
        }

        // Nothing recorded: the next tile that asks tries again.
        guard case .art(let data, let hash) = payload else {
            if case .none = payload { absent.insert(key) }
            return nil
        }
        guard accept(hash: hash, for: key) else { return nil }
        return data
    }

    // MARK: - Keys

    private static func key(_ source: AlbumArtwork.Source) -> String {
        switch source {
        case .album(let id): "album-\(id)"
        case .track(let id): "track-\(id)"
        }
    }

    private static func key(_ source: AlbumArtwork.Source, _ size: AlbumArtwork.Size) -> String {
        "\(key(source))@\(size.rawValue)"
    }

    /// What an attempt learned.
    ///
    /// A server that answers and says it has no art is worth remembering. One
    /// that could not be reached is not: recording that as "no art" is why a
    /// scroll during a blip left permanent holes in the grid until relaunch.
    private enum Fetched {
        case art(Data, hash: String)
        case none
        case failed
    }

    private nonisolated static func fetch(
        _ source: AlbumArtwork.Source,
        _ engine: KoanEngine
    ) async -> Fetched {
        do {
            let data: Data?
            switch source {
            case .album(let albumId):
                // Any track off the record will do — they share the artwork.
                let tracks = try await engine.tracks(
                    albumId: albumId, artistId: nil, sort: .album, limit: 1, offset: 0
                )
                guard let first = tracks.first(where: { $0.path != nil }) ?? tracks.first else {
                    return .none
                }
                data = try await engine.coverArt(trackId: first.id, size: sourcePixels)?.data
            case .track(let trackId):
                data = try await engine.coverArt(trackId: trackId, size: sourcePixels)?.data
            }
            guard let data, !data.isEmpty else { return .none }
            return .art(data, hash: digest(data))
        } catch {
            return .failed
        }
    }

    /// Whether this is real artwork, or the server's placeholder.
    private func accept(hash: String, for key: String) -> Bool {
        if placeholderHashes.contains(hash) {
            absent.insert(key)
            return false
        }

        // Track lookups never teach: a track's cover is its album's cover, so
        // counting it would make every played album look like a repeat.
        guard key.hasPrefix("album-") else { return true }

        hashOwners[hash, default: []].insert(key)
        guard hashOwners[hash]?.count ?? 0 >= 3 else { return true }

        // Three unrelated albums with byte-identical art is a placeholder.
        placeholderHashes.insert(hash)
        absent.formUnion(hashOwners[hash] ?? [])
        return false
    }

    // MARK: - Decoding

    /// Decode off the main actor, downsampled to what the caller will draw.
    ///
    /// `NSImage(data:)` defers the decode until the image is drawn, which puts
    /// it on the main thread in the middle of a scroll — and stored artwork is
    /// a thousand pixels square for a row that shows it at twenty-eight points.
    /// Every size but `.full` comes back as a thumbnail sized for its use.
    private nonisolated static func decode(_ data: Data, to size: AlbumArtwork.Size) -> NSImage? {
        guard let cgSource = CGImageSourceCreateWithData(data as CFData, nil) else {
            return nil
        }

        // The one place the detail shows. Decoded eagerly all the same, so the
        // sheet doesn't do it on the main thread as it animates in.
        guard let limit = size.pixels else {
            guard let full = CGImageSourceCreateImageAtIndex(
                cgSource, 0, [kCGImageSourceShouldCacheImmediately: true] as CFDictionary
            ) else { return nil }
            return NSImage(
                cgImage: full, size: NSSize(width: full.width, height: full.height)
            )
        }

        guard let scaled = CGImageSourceCreateThumbnailAtIndex(
            cgSource, 0,
            [
                kCGImageSourceCreateThumbnailFromImageAlways: true,
                kCGImageSourceCreateThumbnailWithTransform: true,
                kCGImageSourceShouldCacheImmediately: true,
                kCGImageSourceThumbnailMaxPixelSize: limit,
            ] as CFDictionary
        ) else { return nil }
        return NSImage(
            cgImage: scaled, size: NSSize(width: scaled.width, height: scaled.height)
        )
    }

    /// What holding this costs, so the budget is in bytes rather than in
    /// entries — a grid tile is sixteen row thumbnails and should count as it.
    private nonisolated static func cost(_ image: NSImage) -> Int {
        guard let rep = image.representations.first else { return 0 }
        return rep.pixelsWide * rep.pixelsHigh * 4
    }

    // MARK: - Disk

    /// Bumped when the meaning of a cache file changes. Artwork is keyed per
    /// record and stored at one size now; the files written under the old
    /// scheme are per *track* and smaller, so they are wrong twice over and the
    /// directory they live in goes.
    private static let scheme = "v2"

    private static func makeDirectory() -> URL? {
        guard let caches = FileManager.default.urls(
            for: .cachesDirectory, in: .userDomainMask
        ).first else { return nil }
        let root = caches.appendingPathComponent("cc.blit.koan/artwork", isDirectory: true)
        let dir = root.appendingPathComponent(scheme, isDirectory: true)
        do {
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            // Nothing here is authoritative, so an older scheme is deleted
            // rather than migrated. Cheap: it refills as you browse.
            let stale = (try? FileManager.default.contentsOfDirectory(
                at: root, includingPropertiesForKeys: nil
            )) ?? []
            for entry in stale where entry.lastPathComponent != scheme {
                try? FileManager.default.removeItem(at: entry)
            }
            return dir
        } catch {
            return nil
        }
    }

    private nonisolated static func digest(_ data: Data) -> String {
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
        memory.removeAllObjects()
        absent.removeAll()
        hashOwners.removeAll()
        placeholderHashes.removeAll()
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
