#if canImport(AppKit)
import AppKit
#else
import UIKit
#endif
import CryptoKit
import ImageIO
import KoanFFI
import SwiftUI

/// Album art: bytes cached per record, bitmaps cached per record *and* size.
///
/// The two layers are keyed differently on purpose.
///
/// Bytes are fetched and stored once per record. On a remote-backed library
/// every miss is an HTTP round trip, and keyed per track the same
/// sleeve would be fetched once for every track on the record — a dozen requests and a dozen
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
///
/// **Not on the main actor**, which is the load-bearing part. The fetch path
/// has eight awaits in it, so on the main actor every tile would hop there
/// eight times to fetch one cover, and a grid of twenty would queue a hundred
/// and sixty jobs ahead of anything else waiting for an answer. None of this
/// work needs the main actor. The bookkeeping is behind a lock and `NSCache`
/// is thread-safe on its own.
///
/// `Observable` is conformed to by hand rather than synthesised, so views can
/// still reach it through `@Environment`. Nothing on it is observed — a cache
/// is not SwiftUI state — so there is no registrar and nothing to notify.
final class CoverArtCache: Observable, @unchecked Sendable {
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

    private let memory: NSCache<NSString, PlatformImage> = {
        let cache = NSCache<NSString, PlatformImage>()
        cache.totalCostLimit = CoverArtCache.memoryBudget
        return cache
    }()

    /// Records we looked at and found nothing for — distinct from "not looked
    /// yet", which is what keeps missing art from being retried on every
    /// redraw. Checked ahead of `memory`, so a record turning out to be the
    /// server's placeholder invalidates every size of it at once without having
    /// to enumerate them.
    private var absent: Set<String> = []

    /// Guards everything below that is not `NSCache`. Held only across
    /// dictionary reads and writes — never across an await, which is what makes
    /// a plain lock the right primitive rather than an actor.
    private let bookkeeping = NSLock()

    /// One byte fetch per record, shared by every size that wants it.
    private let loads = Flights<Fetched>()
    /// One decode per record and size, shared by everyone on screen asking.
    private let decodes = Flights<PlatformImage?>()

    /// How much each record still being fetched matters to what is on screen,
    /// read by the image lane each time it picks. Raised by whoever asks, and
    /// set outright by a grid aiming its prefetch.
    private var wanted: [String: ImageWork.Priority] = [:]

    /// Which records produced each image, by content hash.
    ///
    /// Navidrome answers with a stock placeholder — a blue vinyl with its own
    /// logo on it — for anything with no artwork, and it looks like real art
    /// until you notice every artless record has the same one. There is no flag
    /// to ask about, but the placeholder gives itself away by repeating across
    /// unrelated albums.
    ///
    /// Only *album* lookups teach this, and it takes three albums to conclude
    /// it. A track's cover is by definition the same image as its album's, so
    /// learning from track lookups would make playing an album look like a
    /// repeat and wipe its real artwork from the grid. Two albums sharing art is
    /// also legitimate — a single and the record it came from — so two
    /// sightings is too eager.
    private var hashOwners: [String: Set<String>] = [:]
    private var placeholderHashes: Set<String> = []

    private let engine: KoanEngine
    private let directory: URL?

    /// The colour space of the display the artwork is drawn on, which is the
    /// one Core Animation composites in. Set by whoever knows the window.
    var displaySpace: CGColorSpace? {
        get { locked { space } }
        set { locked { space = newValue } }
    }
    private var space: CGColorSpace?

    init(engine: KoanEngine) {
        self.engine = engine
        self.directory = Self.makeDirectory()
    }

    // MARK: - Lookup

    /// What's already decoded, without starting anything.
    ///
    /// Views call this first so a cover that has been seen appears in the same
    /// frame rather than after an await, and a scrolled-back grid does not
    /// flash grey.
    func cached(_ source: AlbumArtwork.Source, size: AlbumArtwork.Size) -> PlatformImage? {
        guard !locked({ absent.contains(Self.key(source)) }) else { return nil }
        return memory.object(forKey: Self.key(source, size) as NSString)
    }

    /// Whether the record has been looked at and has no art. `false` is also
    /// a record not looked at yet.
    func isAbsent(_ source: AlbumArtwork.Source) -> Bool {
        locked { absent.contains(Self.key(source)) }
    }

    private func locked<T>(_ body: () -> T) -> T {
        bookkeeping.lock()
        defer { bookkeeping.unlock() }
        return body()
    }

    /// Fetch the art at a given size, awaiting the disk cache or the network as
    /// needed.
    ///
    /// Deliberately `async` and not observable: a read that returned what it
    /// had and started a load as a side effect would make every artwork on
    /// screen depend on every entry in the cache, and one arriving would
    /// invalidate all of them.
    ///
    /// Concurrent callers share work at both layers — a grid and a transport
    /// bar showing the same record at different sizes is one round trip and two
    /// decodes, and forty tiles of the same record is one of each.
    ///
    /// Cancelling the caller gives up its claim. When nobody is left asking,
    /// work that has not started yet is dropped, so a tile scrolled past does
    /// not hold a place in the lane ahead of the ones on screen.
    func image(
        for source: AlbumArtwork.Source,
        size: AlbumArtwork.Size,
        priority: ImageWork.Priority = .visible
    ) async -> PlatformImage? {
        if let ready = cached(source, size: size) { return ready }
        let record = Self.key(source)
        locked { wanted[record] = max(wanted[record] ?? priority, priority) }
        let key = Self.key(source, size)
        return await decodes.join(key, ended: { [weak self] in
            guard let self else { return }
            self.locked { _ = self.wanted.removeValue(forKey: record) }
        }) { [weak self] in
            guard let self, let data = await self.bytes(for: source) else { return nil }
            let space = self.displaySpace
            let decoded = await ImageWork.ifWanted(priority: { [weak self] in self?.priority(of: record) ?? .ahead(.infinity) }) {
                Self.decode(data, to: size, in: space)
            }
            guard let decoded, let image = decoded else { return nil }
            // Full size belongs to whichever sheet asked for it and goes when
            // that sheet does.
            if size != .full {
                self.memory.setObject(image, forKey: key as NSString, cost: Self.cost(image))
            }
            return image
        }
    }

    /// Re-rank records already asked for, as a scroll moves what is on screen
    /// and where it is heading. Records nobody has asked for are left alone.
    func prioritise(_ priorities: [(AlbumArtwork.Source, ImageWork.Priority)]) {
        locked {
            for (source, priority) in priorities {
                let record = Self.key(source)
                if wanted[record] != nil { wanted[record] = priority }
            }
        }
    }

    private func priority(of record: String) -> ImageWork.Priority {
        locked { wanted[record] ?? .ahead(.infinity) }
    }

    /// The colour a record reads as.
    ///
    /// Shares the byte fetch with the artwork itself, so the wash costs no
    /// extra round trip, and does its pixel work on `ImageWork` rather than
    /// wherever it was asked from.
    func dominantColour(for source: AlbumArtwork.Source) async -> Color? {
        let key = Self.key(source)
        if let known = locked({ colours[key] }) { return known }
        guard let data = await bytes(for: source) else { return nil }
        let colour = await ImageWork.onCPU { Color.dominant(ofEncoded: data) }
        locked { colours[key] = colour }
        return colour
    }

    /// The colour if it has already been worked out.
    ///
    /// For a caller that has to answer in the frame it is asked — nothing is
    /// fetched and nothing is decoded. The outer `nil` means "not yet"; the
    /// inner one a record known to have no art, or art with no colour.
    func cachedColour(for source: AlbumArtwork.Source) -> Color?? {
        let key = Self.key(source)
        return locked { absent.contains(key) ? .some(nil) : colours[key] }
    }

    /// Remembered per record: the wash re-asks every time you navigate, and the
    /// answer cannot change without the artwork changing.
    private var colours: [String: Color?] = [:]

    // MARK: - Bytes

    /// The record's encoded artwork, from disk or the network. One fetch per
    /// record however many sizes are waiting on it.
    private func bytes(for source: AlbumArtwork.Source) async -> Data? {
        let key = Self.key(source)
        guard !locked({ absent.contains(key) }) else { return nil }

        let engine = self.engine
        let file = directory?.appendingPathComponent(Self.filename(for: key))
        let priority: @Sendable () -> ImageWork.Priority = { [weak self] in self?.priority(of: key) ?? .ahead(.infinity) }
        let payload = await loads.join(key) {
            guard !Task.isCancelled else { return .failed }
            // Every step off both the main actor and the cooperative pool —
            // see `ImageWork`. The hash comes back with the bytes because it
            // is a pass over the whole payload.
            if let file, let cached = await ImageWork.onDisk({ try? Data(contentsOf: file) }) {
                guard let hash = await ImageWork.ifWanted(priority: priority, { Self.digest(cached) }) else {
                    return .failed
                }
                return .art(cached, hash: hash)
            }
            let fetched = await Self.fetch(source, engine, priority: priority)
            if case .art(let bytes, _) = fetched, let file {
                // Best effort, and not waited for: the sleeve is wanted on
                // screen now, and a cache that fails to write is still a cache.
                ImageWork.inBackground { try? bytes.write(to: file, options: .atomic) }
            }
            return fetched
        }

        // Nothing recorded: the next tile that asks tries again.
        guard case .art(let data, let hash) = payload else {
            if case .none = payload { locked { _ = absent.insert(key) } }
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
        case .artist(let id): "artist-\(id)"
        }
    }

    private static func key(_ source: AlbumArtwork.Source, _ size: AlbumArtwork.Size) -> String {
        "\(key(source))@\(size.rawValue)"
    }

    /// What an attempt learned.
    ///
    /// A server that answers and says it has no art is worth remembering. One
    /// that could not be reached is not: recorded as "no art", a scroll during
    /// a blip would leave permanent holes in the grid until relaunch.
    private enum Fetched: Sendable {
        case art(Data, hash: String)
        case none
        case failed
    }

    private nonisolated static func fetch(
        _ source: AlbumArtwork.Source,
        _ engine: KoanEngine,
        priority: @escaping @Sendable () -> ImageWork.Priority
    ) async -> Fetched {
        do {
            // One call either way. Asking for the record's tracks to find an id
            // to ask for art with would build a listing and carry it across the
            // boundary to be thrown away — once per tile, on a grid of them.
            let data: Data?
            switch source {
            case .album(let albumId):
                data = try await engine.albumCoverArt(albumId: albumId, size: sourcePixels)?.data
            case .track(let trackId):
                data = try await engine.coverArt(trackId: trackId, size: sourcePixels)?.data
            case .artist(let artistId):
                data = try await engine.artistImage(artistId: artistId)?.data
            }
            guard let data, !data.isEmpty else { return .none }
            guard let hash = await ImageWork.ifWanted(priority: priority, { digest(data) }) else { return .failed }
            return .art(data, hash: hash)
        } catch {
            return .failed
        }
    }

    /// Whether this is real artwork, or the server's placeholder.
    private func accept(hash: String, for key: String) -> Bool {
        locked { acceptLocked(hash: hash, for: key) }
    }

    private func acceptLocked(hash: String, for key: String) -> Bool {
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
    /// A platform image built from `Data` defers the decode until it is drawn,
    /// which puts it on the main thread in the middle of a scroll — and stored
    /// artwork is a thousand pixels square for a row that shows it at
    /// twenty-eight points.
    /// Every size but `.full` comes back as a thumbnail sized for its use.
    private nonisolated static func decode(
        _ data: Data,
        to size: AlbumArtwork.Size,
        in space: CGColorSpace?
    ) -> PlatformImage? {
        guard let cgSource = CGImageSourceCreateWithData(data as CFData, nil) else {
            return nil
        }

        // The one place the detail shows. Decoded eagerly all the same, so the
        // sheet doesn't do it on the main thread as it animates in.
        guard let limit = size.pixels else {
            guard let full = CGImageSourceCreateImageAtIndex(
                cgSource, 0, [kCGImageSourceShouldCacheImmediately: true] as CFDictionary
            ) else { return nil }
            return .decoded(full, pixelSize: CGSize(width: full.width, height: full.height))
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
        let native = redrawn(scaled, in: space) ?? scaled
        return .decoded(native, pixelSize: CGSize(width: native.width, height: native.height))
    }

    /// The bitmap in the format Core Animation draws without converting:
    /// 32-bit BGRA, premultiplied, in the display's colour space.
    ///
    /// A JPEG decodes to 24-bit RGB in whatever colour space it was tagged
    /// with. A layer handed that converts it to its own format and to the
    /// display's colour space when the transaction commits, on the main thread
    /// — in a scroll, every sleeve as it arrives, and the largest single cost
    /// of a frame. Done here, it is done once, off the main thread, and the
    /// commit has nothing to do. Without a display to match, sRGB.
    private nonisolated static func redrawn(_ image: CGImage, in display: CGColorSpace?) -> CGImage? {
        guard let space = display ?? CGColorSpace(name: CGColorSpace.sRGB),
              let context = CGContext(
                  data: nil, width: image.width, height: image.height,
                  bitsPerComponent: 8, bytesPerRow: 0, space: space,
                  bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue
              )
        else { return nil }
        context.interpolationQuality = .none
        context.draw(image, in: CGRect(x: 0, y: 0, width: image.width, height: image.height))
        return context.makeImage()
    }

    /// What holding this costs, so the budget is in bytes rather than in
    /// entries — a grid tile is sixteen row thumbnails and should count as it.
    private nonisolated static func cost(_ image: PlatformImage) -> Int {
        // Read out of the representation rather than through `bitmap`, which on
        // AppKit is allowed to rasterise to answer.
        #if canImport(AppKit)
        guard let rep = image.representations.first else { return 0 }
        return rep.pixelsWide * rep.pixelsHigh * 4
        #else
        guard let bitmap = image.cgImage else { return 0 }
        return bitmap.width * bitmap.height * 4
        #endif
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
        // An instance on a configuration of its own (`KOAN_CONFIG_DIR`: a test
        // library, the evidence renderer) keeps its artwork beside it. Files
        // are named by record id, and ids from another library would show
        // that library's covers in this one.
        let root = ProcessInfo.processInfo.environment["KOAN_CONFIG_DIR"]
            .map { URL(fileURLWithPath: $0).appendingPathComponent("artwork", isDirectory: true) }
            ?? caches.appendingPathComponent("cc.blit.koan/artwork", isDirectory: true)
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
        digest(Data(key.utf8))
    }

    /// Forget what is cached for records the library has deleted since last
    /// asked. SQLite gives a freed id to the next row it inserts, so a cover
    /// cached under an old album's id would otherwise be shown for a new one.
    /// Records whose cover image on disk may have changed are named too.
    func applyEvictions() async {
        // Per configuration, as the files are.
        let mark = ProcessInfo.processInfo.environment["KOAN_CONFIG_DIR"].map { "artEvictionSeq-\($0)" }
            ?? "artEvictionSeq"
        let after = Int64(UserDefaults.standard.integer(forKey: mark))
        guard let gone = try? await engine.artEvictions(after: after) else { return }
        let sources: [AlbumArtwork.Source] =
            gone.albums.map { .album($0) } + gone.artists.map { .artist($0) }
            + gone.tracks.map { .track($0) }
        for source in sources { evict(source) }
        UserDefaults.standard.set(Int(gone.seq), forKey: mark)
    }

    private func evict(_ source: AlbumArtwork.Source) {
        let key = Self.key(source)
        for size in [AlbumArtwork.Size.thumb, .tile, .full] {
            memory.removeObject(forKey: Self.key(source, size) as NSString)
        }
        locked {
            absent.remove(key)
            colours.removeValue(forKey: key)
        }
        if let file = directory?.appendingPathComponent(Self.filename(for: key)) {
            try? FileManager.default.removeItem(at: file)
        }
    }

    /// Drop everything. Exposed for settings; the system may also reclaim the
    /// directory on its own.
    func purge() {
        memory.removeAllObjects()
        locked {
            absent.removeAll()
            hashOwners.removeAll()
            placeholderHashes.removeAll()
            colours.removeAll()
        }
        guard let directory else { return }
        try? FileManager.default.removeItem(at: directory)
        _ = Self.makeDirectory()
    }
}

/// Work shared by everyone asking for the same thing, and abandoned when the
/// last of them stops asking.
///
/// An unstructured task does not inherit its callers' cancellation, so one
/// shared between them would run to the end for a tile nobody can see. Each
/// caller holds a claim instead; the work is cancelled when the claims run out,
/// and a caller arriving after that starts it afresh.
private final class Flights<Value: Sendable>: @unchecked Sendable {
    private struct Flight {
        let task: Task<Value, Never>
        var claims: Set<UUID>
        let ended: (@Sendable () -> Void)?
    }

    private let lock = NSLock()
    private var flights: [String: Flight] = [:]

    /// `ended` runs once nobody is waiting on the work any longer, finished
    /// or not.
    func join(
        _ key: String,
        ended: (@Sendable () -> Void)? = nil,
        _ work: @escaping @Sendable () async -> Value
    ) async -> Value {
        let claim = UUID()
        let task: Task<Value, Never> = lock.withLock {
            if var flight = flights[key] {
                flight.claims.insert(claim)
                flights[key] = flight
                return flight.task
            }
            let task = Task { await work() }
            flights[key] = Flight(task: task, claims: [claim], ended: ended)
            return task
        }
        let value = await withTaskCancellationHandler {
            await task.value
        } onCancel: {
            self.release(claim, on: key, of: task)
        }
        release(claim, on: key, of: task)
        return value
    }

    private func release(_ claim: UUID, on key: String, of task: Task<Value, Never>) {
        let ended: (@Sendable () -> Void)?? = lock.withLock {
            guard var flight = flights[key], flight.task == task,
                  flight.claims.remove(claim) != nil
            else { return nil }
            guard flight.claims.isEmpty else {
                flights[key] = flight
                return nil
            }
            flights[key] = nil
            return .some(flight.ended)
        }
        guard let ended else { return }
        task.cancel()
        ended?()
    }
}
