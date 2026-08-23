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

    /// Everything the transport needs, refreshed every tick.
    ///
    /// Read this ONLY where a ticking value is actually wanted. `positionMs`
    /// changes ten times a second, so a view that reads any field of this
    /// struct re-renders at that rate — which in a list means rows are being
    /// replaced under the pointer and clicks get dropped. Lists should read the
    /// derived properties below, which are only assigned when they change.
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
        let now = engine.nowPlaying()
        nowPlaying = now
        updateDerived(now)
        // The queue only gets rebuilt when the engine says it changed.
        if nowPlaying.playlistVersion != knownQueueVersion {
            knownQueueVersion = nowPlaying.playlistVersion
            rebuildQueue()
        } else if hasActiveDownloads, downloadRefreshDue {
            lastDownloadRefresh = Date.now
            // Download progress moves without bumping the playlist version, so
            // a version check alone would freeze the progress bars. But
            // replacing the queue array is what the List diffs against, and
            // doing it ten times a second cancels drags mid-gesture and eats
            // clicks. Once a second is plenty for a progress bar.
            rebuildQueue()
        }
        settlePendingSeek()
        onTick?()
    }

    /// Where a seek asked to land, until the engine reports being near it.
    private var pendingSeekMs: UInt64?
    private var pendingSeekTicks = 0

    /// Release the held position once the engine has caught up — or give up, so
    /// a seek the engine rejected can't wedge the bar permanently.
    private func settlePendingSeek() {
        guard let target = pendingSeekMs else { return }
        let reached = abs(Int64(nowPlaying.positionMs) - Int64(target)) < 750
        pendingSeekTicks += 1
        if reached || pendingSeekTicks > 20 {
            pendingSeekMs = nil
            pendingSeekTicks = 0
            scrubbing = nil
        }
    }

    private var hasActiveDownloads = false
    private var lastDownloadRefresh = Date.distantPast

    private var downloadRefreshDue: Bool {
        Date.now.timeIntervalSince(lastDownloadRefresh) >= 1
    }

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
    //
    // Stored rather than computed, and assigned only on an actual change, so
    // observers of these don't inherit the tick rate of `nowPlaying`.

    private(set) var isPlaying = false
    private(set) var currentTrackId: Int64?
    private(set) var currentItemId: String?
    private(set) var radioEnabled = false
    /// Whole seconds. Anything that only needs "roughly where are we" — lyric
    /// highlighting, for instance — should read this rather than `positionMs`,
    /// so it re-renders once a second instead of ten times.
    private(set) var positionSeconds = 0

    private func updateDerived(_ now: NowPlaying) {
        let playing = now.state == .playing
        if playing != isPlaying { isPlaying = playing }
        if now.entry?.trackId != currentTrackId { currentTrackId = now.entry?.trackId }
        if now.queueItemId != currentItemId { currentItemId = now.queueItemId }
        if now.radioEnabled != radioEnabled { radioEnabled = now.radioEnabled }
        let seconds = Int(now.positionMs / 1000)
        if seconds != positionSeconds { positionSeconds = seconds }
    }

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

    /// Called as the thumb is dragged. Cancels any seek still settling, since
    /// the user is now the authority on where the head is.
    func beginScrub(fraction: Double) {
        pendingSeekMs = nil
        pendingSeekTicks = 0
        scrubbing = min(1, max(0, fraction))
    }

    /// Nudge by a number of seconds, clamped to the track. What the arrow-key
    /// shortcuts and the TUI's `,`/`.` do.
    func seek(bySeconds delta: Int) {
        let current = Int64(nowPlaying.positionMs)
        let target = max(0, current + Int64(delta) * 1000)
        seek(toMs: UInt64(min(target, Int64(nowPlaying.durationMs))))
    }

    /// Favourite whatever is playing. No-op when the queue item has no library
    /// row behind it.
    @discardableResult
    func toggleFavouriteCurrent() -> Bool {
        guard let trackId = currentTrackId else { return false }
        return toggleFavourite(trackId: trackId)
    }

    /// Hold the requested position until the engine agrees with it.
    ///
    /// The seek is asynchronous — it goes down a channel to the player thread,
    /// which restarts decoding before `position_ms` moves. Clearing the local
    /// value when the command is merely *sent* hands the bar back to the engine
    /// during that gap, so the poll reads the old position and the thumb snaps
    /// backwards before jumping forward again.
    func seek(toMs ms: UInt64) {
        pendingSeekMs = ms
        if nowPlaying.durationMs > 0 {
            scrubbing = Double(ms) / Double(nowPlaying.durationMs)
        }
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

    /// Flips it without the caller having to read the current value — menus
    /// that read observable state rebuild themselves constantly.
    func toggleRadio() { setRadio(!nowPlaying.radioEnabled) }

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
