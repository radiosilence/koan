import KoanFFI
import SwiftUI

/// Everything koan is fetching, and what it fetched a moment ago.
///
/// One store rather than a listener per track. The engine broadcasts the whole
/// in-flight set several times a second, so there is nothing to register when a
/// download starts and nothing to unregister when it ends — and skipping
/// between three downloading tracks and back cannot lose one, because every
/// message carries all of them.
///
/// It outlives the queue's own knowledge on purpose. A queue item stops saying
/// it is downloading the moment it lands, which is exactly when someone wants
/// to see that it did.
@MainActor
@Observable
final class DownloadsModel {
    struct Item: Identifiable {
        let id: String
        var trackId: Int64?
        var title: String
        var artist: String
        /// 0–1, or `nil` for a transfer whose length the server never gave.
        var progress: Double?
        var state: State

        var isActive: Bool { state == .running }
    }

    enum State: Equatable {
        case running
        case finished
        case failed(String)
    }

    /// Running first, then whatever has just settled, newest first.
    private(set) var items: [Item] = []

    /// How many transfers are going. What the sidebar counts.
    var activeCount: Int { items.lazy.filter(\.isActive).count }

    /// Kept small: this is a view of now, not an archive. Old entries are the
    /// least interesting thing on the page and would push the live ones off it.
    private static let settledLimit = 40

    /// The whole in-flight set, as the engine last reported it, joined against
    /// the queue for names.
    ///
    /// Anything previously running and now absent has settled — the queue is
    /// asked which way, since a failure leaves a reason on the entry and a
    /// success leaves nothing at all.
    func apply(_ downloads: [DownloadProgress], queue: [QueueItem]) {
        let named = Dictionary(queue.map { ($0.queueItemId, $0) }, uniquingKeysWith: { a, _ in a })
        let running = Set(downloads.map(\.queueItemId))

        for index in items.indices where items[index].state == .running {
            guard !running.contains(items[index].id) else { continue }
            items[index].state = settled(items[index].id, in: named)
            items[index].progress = items[index].state == .finished ? 1 : items[index].progress
        }

        for download in downloads {
            let entry = named[download.queueItemId]
            if let index = items.firstIndex(where: { $0.id == download.queueItemId }) {
                items[index].progress = download.progress
                items[index].state = .running
                // A name can arrive after the transfer does — the queue is
                // fetched on its own schedule.
                if let entry {
                    items[index].title = entry.title
                    items[index].artist = entry.artist
                    items[index].trackId = entry.trackId
                }
            } else {
                items.insert(
                    Item(
                        id: download.queueItemId,
                        trackId: entry?.trackId,
                        title: entry?.title ?? "Unknown track",
                        artist: entry?.artist ?? "",
                        progress: download.progress,
                        state: .running
                    ),
                    at: 0
                )
            }
        }

        sortAndTrim()
    }

    /// Forget what has already settled. The running ones are not this button's
    /// business — stopping a transfer is a different verb.
    func clearSettled() {
        items.removeAll { $0.state != .running }
    }

    private func settled(_ id: String, in named: [String: QueueItem]) -> State {
        guard let entry = named[id] else { return .finished }
        if entry.status == .failed {
            return .failed(entry.failureReason ?? "Couldn't be fetched")
        }
        return .finished
    }

    private func sortAndTrim() {
        // A stable partition: running rises, and neither half is reordered, so
        // a list being watched does not shuffle under the pointer.
        let running = items.filter(\.isActive)
        var settled = items.filter { !$0.isActive }
        if settled.count > Self.settledLimit {
            settled = Array(settled.prefix(Self.settledLimit))
        }
        items = running + settled
    }
}
