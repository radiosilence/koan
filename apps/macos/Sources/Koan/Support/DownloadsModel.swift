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

    /// What the sidebar counts.
    ///
    /// Its own property rather than a count over `items`. Reading it there
    /// meant the sidebar re-rendered every time the list was refreshed, which
    /// while anything is downloading is ten times a second — for a number that
    /// changes when a transfer starts or ends and at no other time.
    private(set) var activeCount = 0

    var hasSettled: Bool {
        items.contains { $0.state == .done || $0.state == .failed }
    }

    init(engine: KoanEngine) {
        self.engine = engine
    }

    /// The list changed shape. Refetch it.
    ///
    /// Called when the store's version moves, and on a timer while the
    /// downloads page is actually on screen — see `watch()`. Not on every
    /// progress event: assigning this array is what tells SwiftUI the list
    /// changed, and doing that ten times a second re-rendered everything
    /// reading it whether or not anyone was looking at a download.
    func reload() {
        let fresh = engine.downloads()
        if fresh.count != items.count
            || zip(fresh, items).contains(where: { !$0.matches($1) })
        {
            items = fresh
        }
        let active = fresh.lazy.filter { $0.state == .queued || $0.state == .running }.count
        if active != activeCount { activeCount = active }
    }

    /// Byte counts moved.
    ///
    /// Only the count is taken. The figures behind it are read from the store,
    /// and only while somebody is looking at them — this fires ten times a
    /// second for as long as a transfer runs, and it is the busiest thing in
    /// the app when one does.
    func applyProgress(_ downloads: [DownloadProgress]) {
        if downloads.count != activeCount { activeCount = downloads.count }
    }

    /// Keep `items` fresh for as long as the caller is showing them. Ends when
    /// the task is cancelled, which is when the page goes away.
    func watch() async {
        while !Task.isCancelled {
            reload()
            try? await Task.sleep(for: .milliseconds(100))
        }
    }

    func clearSettled() {
        engine.clearSettledDownloads()
        reload()
    }
}

private extension DownloadEntry {
    /// Whether two readings of one transfer say the same thing. Compared field
    /// by field because the generated record has no equality of its own, and
    /// because assigning an unchanged array is still a change to SwiftUI.
    func matches(_ other: DownloadEntry) -> Bool {
        queueItemId == other.queueItemId
            && state == other.state
            && bytesWritten == other.bytesWritten
            && bytesPerSecond == other.bytesPerSecond
            && progress == other.progress
    }
}
