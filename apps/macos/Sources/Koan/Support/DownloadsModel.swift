import KoanFFI
import SwiftUI

/// Everything koan is fetching, and what it fetched a moment ago.
///
/// A mirror, not a store. The engine owns the list — it is the only thing that
/// knows a transfer started, and it goes on knowing after one lands, which is
/// exactly when somebody wants to see that it did. This holds the last snapshot
/// so views have something to draw between events.
///
/// Two events feed it, because two things move at different rates. The list
/// changes when a transfer appears or settles, which is rare; the byte counts
/// change hundreds of times a second. Rebuilding the list on the second would
/// mean rebuilding it at the rate the download writes.
@MainActor
@Observable
final class DownloadsModel {
    private let engine: KoanEngine

    private(set) var items: [DownloadEntry] = []

    /// What the sidebar counts. Zero most of the time.
    var activeCount: Int {
        items.lazy.filter { $0.state == .queued || $0.state == .running }.count
    }

    var hasSettled: Bool {
        items.contains { $0.state == .done || $0.state == .failed }
    }

    init(engine: KoanEngine) {
        self.engine = engine
    }

    /// The list changed shape. Refetch it.
    func reload() {
        items = engine.downloads()
    }

    /// Byte counts moved. The rows are the same rows, so only the figures are
    /// taken — replacing the array wholesale would animate every row on every
    /// tick of a transfer.
    func applyProgress(_ downloads: [DownloadProgress]) {
        guard !items.isEmpty else { return }
        let byItem = Dictionary(
            downloads.map { ($0.queueItemId, $0.progress) },
            uniquingKeysWith: { first, _ in first }
        )
        for index in items.indices {
            guard let progress = byItem[items[index].queueItemId] else { continue }
            guard items[index].progress != progress else { continue }
            items[index].progress = progress
        }
    }

    func clearSettled() {
        engine.clearSettledDownloads()
        reload()
    }
}
