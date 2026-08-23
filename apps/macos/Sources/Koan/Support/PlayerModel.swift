import Foundation
import KoanFFI

/// Playback state, mirrored from the Rust engine.
///
/// The engine owns the truth and the UI polls it — the same contract the TUI
/// works under. Polling beats a callback bridge here: the state we want is a
/// handful of atomics behind an `Arc`, reading it is cheaper than the FFI call
/// that would deliver a notification, and a dropped tick costs us nothing.
@MainActor
@Observable
final class PlayerModel {
    let engine: KoanEngine

    private(set) var nowPlaying: NowPlaying
    private(set) var queue: [QueueItem] = []
    /// Queue entries indexed by library track id, so a library row can show
    /// what the queue knows about it — downloading, failed, already played —
    /// without scanning the queue once per row.
    private(set) var queuedByTrack: [Int64: QueueItem] = [:]
    private(set) var devices: [Device] = []
    /// `nil` means system default. Read back from config, so it survives restarts.
    private(set) var currentDevice: String?

    /// Set while the user drags the seek head, so the poll doesn't yank the
    /// thumb back to the engine's position mid-gesture.
    var scrubbing: Double?
    var lastError: String?

    private var knownQueueVersion: UInt64 = .max
    private var ticker: Task<Void, Never>?

    /// Run after every state read. Now Playing hangs off this rather than
    /// polling the engine a second time on its own timer.
    var onTick: (() -> Void)?

    init(engine: KoanEngine) {
        self.engine = engine
        self.nowPlaying = engine.nowPlaying()
    }

    /// 10 Hz — smooth enough for a seek bar, far below anything the engine notices.
    func startPolling() {
        guard ticker == nil else { return }
        ticker = Task { [weak self] in
            while !Task.isCancelled {
                self?.tick()
                try? await Task.sleep(for: .milliseconds(100))
            }
        }
        refreshDevices()
    }

    private func tick() {
        nowPlaying = engine.nowPlaying()
        // The queue only gets rebuilt when the engine says it changed.
        if nowPlaying.playlistVersion != knownQueueVersion {
            knownQueueVersion = nowPlaying.playlistVersion
            rebuildQueue()
        } else if hasActiveDownloads {
            // Download progress moves without bumping the playlist version, so
            // a version check alone would freeze the progress bars. Only worth
            // re-reading while something is actually in flight — rebuilding the
            // whole queue every tick is the thing the version check exists to
            // avoid.
            rebuildQueue()
        }
        onTick?()
    }

    private var hasActiveDownloads = false

    private func rebuildQueue() {
        queue = engine.queue()
        queuedByTrack = Dictionary(
            queue.compactMap { item in item.trackId.map { ($0, item) } },
            // A track queued twice: prefer the entry that is actually doing
            // something over one still sitting idle.
            uniquingKeysWith: { a, b in b.status == .queued ? a : b }
        )
        hasActiveDownloads = queue.contains {
            $0.status == .downloading || $0.status == .priorityPending
        }
    }

    // MARK: - Derived

    var isPlaying: Bool { nowPlaying.state == .playing }
    var currentTrackId: Int64? { nowPlaying.entry?.trackId }

    /// 0–1 through the current track. Reflects the drag while scrubbing.
    var progress: Double {
        if let scrubbing { return scrubbing }
        guard nowPlaying.durationMs > 0 else { return 0 }
        return min(1, Double(nowPlaying.positionMs) / Double(nowPlaying.durationMs))
    }

    var upNext: [QueueItem] {
        guard let cursor = nowPlaying.queueItemId,
              let index = queue.firstIndex(where: { $0.queueItemId == cursor })
        else { return queue }
        return Array(queue.dropFirst(index + 1))
    }

    // MARK: - Transport

    func togglePlayPause() { attempt { try engine.togglePlayPause() } }
    func pause() { attempt { try engine.pause() } }
    func resume() { attempt { try engine.resume() } }
    func next() { attempt { try engine.next() } }
    func previous() { attempt { try engine.previous() } }
    func stop() { attempt { try engine.stop() } }

    func play(itemId: String) { attempt { try engine.play(queueItemId: itemId) } }

    /// Commit a scrub. Position comes from the drag, not the engine.
    func seek(fraction: Double) {
        seek(toMs: UInt64(max(0, min(1, fraction)) * Double(nowPlaying.durationMs)))
    }

    func seek(toMs ms: UInt64) {
        scrubbing = nil
        attempt { try engine.seek(positionMs: ms) }
    }

    // MARK: - Queue

    /// Replace the queue and start playing — double-clicking an album.
    func playNow(trackIds: [Int64]) {
        guard !trackIds.isEmpty else { return }
        attempt { _ = try engine.replaceQueue(trackIds: trackIds) }
    }

    /// Queue the whole list but start at the track that was clicked, so the rest
    /// of the album still plays after it. `replaceQueue` hands back the new item
    /// IDs in order, which is what makes the jump addressable.
    func playNow(trackIds: [Int64], startingAt index: Int) {
        guard trackIds.indices.contains(index) else { return playNow(trackIds: trackIds) }
        attempt {
            let itemIds = try engine.replaceQueue(trackIds: trackIds)
            if itemIds.indices.contains(index) {
                try engine.play(queueItemId: itemIds[index])
            }
        }
    }

    func enqueue(trackIds: [Int64]) {
        guard !trackIds.isEmpty else { return }
        attempt { _ = try engine.addToQueue(trackIds: trackIds) }
    }

    func remove(itemIds: [String]) {
        guard !itemIds.isEmpty else { return }
        attempt { try engine.removeFromQueue(queueItemIds: itemIds) }
    }

    func move(itemIds: [String], after target: String) {
        attempt { try engine.moveInQueue(queueItemIds: itemIds, targetQueueItemId: target, after: true) }
    }

    func clearQueue() { attempt { try engine.clearQueue() } }
    func undo() { attempt { try engine.undo() } }
    func redo() { attempt { try engine.redo() } }

    // MARK: - Devices & modes

    func refreshDevices() {
        let engine = self.engine
        Task {
            let found = await Task.detached(priority: .utility) {
                (try? engine.devices()) ?? []
            }.value
            self.devices = found
            self.currentDevice = engine.currentDevice()
        }
    }

    func setDevice(_ name: String?) {
        attempt {
            if let name { try engine.setDevice(name: name) } else { try engine.clearDevice() }
        }
        currentDevice = name
    }

    func setRadio(_ enabled: Bool) { engine.setRadio(enabled: enabled) }

    /// Toggles, and returns nothing — the library view refetches to pick it up.
    @discardableResult
    func toggleFavourite(trackId: Int64) -> Bool {
        (try? engine.toggleFavourite(trackId: trackId)) ?? false
    }

    // MARK: - Session

    /// Persist the queue and position. Called on quit, and periodically so an
    /// unclean exit doesn't lose the session.
    func saveSession() {
        try? engine.saveSession()
    }

    /// Restore the queue from the last session without starting playback.
    func restoreSession() {
        let engine = self.engine
        Task {
            _ = await Task.detached(priority: .userInitiated) {
                try? engine.restoreSession()
            }.value
        }
    }

    // MARK: - Errors

    /// Engine calls fail for real reasons (device vanished, track gone) but
    /// none of them are worth a modal. Surface it and carry on.
    private func attempt(_ body: () throws -> Void) {
        do {
            try body()
        } catch {
            lastError = String(describing: error)
        }
    }
}
