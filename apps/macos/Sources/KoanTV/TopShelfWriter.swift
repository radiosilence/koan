import CryptoKit
import Foundation
import ImageIO
import KoanFFI
import TVServices
import UniformTypeIdentifiers

/// Keeps the Top Shelf's snapshot in step with the library: rewritten when
/// the library, its play history or the favourite records change, and once at
/// launch, compared against what is on disk rather than what was last
/// written, since tvOS may have emptied the cache in between.
@MainActor
final class TopShelfWriter {
    private weak var state: AppState?
    /// A write is running or waiting to; changes meanwhile are folded into it.
    private var running = false
    private var dirty = false

    /// Changes come in bursts: a sync bumps the library many times, and a
    /// track starting records a play and moves the library together.
    private static let settle: Duration = .seconds(2)
    /// Sleeves are drawn at about 400 points, 800 pixels on a 4K television.
    private nonisolated static let pixels = 800

    private init(state: AppState) {
        self.state = state
    }

    /// Follows the app for as long as it runs.
    static func start(_ state: AppState) {
        let writer = TopShelfWriter(state: state)
        state.mirror.follow { [weak state] in
            guard let state else { return }
            _ = state.mirror.libraryVersion
            _ = state.mirror.historyVersion
            _ = state.library.favouriteAlbumIds
            writer.changed()
        }
    }

    private func changed() {
        dirty = true
        guard !running else { return }
        running = true
        Task {
            while dirty {
                try? await Task.sleep(for: Self.settle)
                dirty = false
                await write()
            }
            running = false
        }
    }

    private func write() async {
        guard let state else { return }
        // Without the group entitlement, as in an unsigned build.
        guard let directory = TopShelfSnapshot.directory else {
            state.engine.logNote(message: "top shelf: no app group container")
            return
        }
        let shelf: TopShelf
        do {
            shelf = try await state.engine.topShelf()
        } catch {
            state.engine.logNote(message: "top shelf: \(error)")
            return
        }
        do {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        } catch {
            state.engine.logNote(message: "top shelf: \(error)")
            return
        }
        let previous = TopShelfSnapshot.read(from: directory)
        // A sleeve that could not be fetched this time keeps the one it had.
        var kept: [String: String] = [:]
        for item in previous?.sections.flatMap(\.items) ?? [] { kept[item.uid] = item.image }

        var images: [String: String] = [:]
        func item(_ record: TopShelfRecord) async -> TopShelfSnapshot.Item? {
            if images[record.uid] == nil {
                images[record.uid] = await image(of: record, in: directory, state: state) ?? kept[record.uid]
            }
            guard let image = images[record.uid],
                  FileManager.default.fileExists(atPath: directory.appending(path: image).path)
            else { return nil }
            return .init(uid: record.uid, title: record.album.title, image: image)
        }
        var recent: [TopShelfSnapshot.Item] = []
        for record in shelf.recent { if let i = await item(record) { recent.append(i) } }
        var favourites: [TopShelfSnapshot.Item] = []
        for record in shelf.favourites { if let i = await item(record) { favourites.append(i) } }

        let snapshot = TopShelfSnapshot(sections: [
            .init(title: "Recently Played", items: recent),
            .init(title: "Favourites", items: favourites),
        ].filter { !$0.items.isEmpty })

        if snapshot != previous {
            do {
                try JSONEncoder().encode(snapshot)
                    .write(to: directory.appending(path: TopShelfSnapshot.file), options: .atomic)
            } catch {
                state.engine.logNote(message: "top shelf: \(error)")
                return
            }
            TVTopShelfContentProvider.topShelfContentDidChange()
        }
        // The previous snapshot's too: the system may still be drawing it
        // until it has asked the extension again.
        let listed = Set([snapshot, previous].compactMap(\.self).flatMap(\.sections).flatMap(\.items).map(\.image))
        let files = (try? FileManager.default.contentsOfDirectory(atPath: directory.path)) ?? []
        for file in files where file != TopShelfSnapshot.file && !listed.contains(file) {
            try? FileManager.default.removeItem(at: directory.appending(path: file))
        }
    }

    /// The record's sleeve as a JPEG at the shelf's size, named by the art it
    /// was made from: new art is a new name, so neither this nor the system's
    /// own cache of the shelf keeps showing the old one, and art already
    /// written is not made again on every pass.
    private func image(of record: TopShelfRecord, in directory: URL, state: AppState) async -> String? {
        guard let data = await state.art.bytes(for: .album(record.album.id)) else { return nil }
        return await Task.detached(priority: .utility) {
            let digest = SHA256.hash(data: data).prefix(12).map { String(format: "%02x", $0) }.joined()
            let name = "\(digest).jpg"
            let file = directory.appending(path: name)
            if FileManager.default.fileExists(atPath: file.path) { return name }
            guard let jpeg = Self.thumbnail(data), (try? jpeg.write(to: file, options: .atomic)) != nil
            else { return nil }
            return name
        }.value
    }

    private nonisolated static func thumbnail(_ data: Data) -> Data? {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil),
              let image = CGImageSourceCreateThumbnailAtIndex(source, 0, [
                  kCGImageSourceCreateThumbnailFromImageAlways: true,
                  kCGImageSourceCreateThumbnailWithTransform: true,
                  kCGImageSourceThumbnailMaxPixelSize: pixels,
              ] as CFDictionary)
        else { return nil }
        let out = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(out, UTType.jpeg.identifier as CFString, 1, nil)
        else { return nil }
        CGImageDestinationAddImage(destination, image, [kCGImageDestinationLossyCompressionQuality: 0.85] as CFDictionary)
        guard CGImageDestinationFinalize(destination) else { return nil }
        return out as Data
    }
}
