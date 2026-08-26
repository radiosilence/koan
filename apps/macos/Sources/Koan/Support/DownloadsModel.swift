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

    /// Byte counts moved. Re-read them from the store.
    ///
    /// The event carries figures of its own, derived from the queue, but taking
    /// them would mean two accounts of one transfer that have to agree — and
    /// the store already holds a live counter the downloader writes. So the
    /// event is used only as a tick, and the numbers come from the one place
    /// that has them. Half a dozen rows of small strings; the copy is nothing.
    func applyProgress(_ downloads: [DownloadProgress]) {
        guard !items.isEmpty || !downloads.isEmpty else { return }
        reload()
    }

    func clearSettled() {
        engine.clearSettledDownloads()
        reload()
    }
}
