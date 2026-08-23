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

    /// The last raw snapshot. Deliberately not observed: it carries a position
    /// that moves every tick, and anything observing it would re-render at that
    /// rate. Views read the change-guarded properties below, or `clock`.
    @ObservationIgnored private(set) var nowPlaying: NowPlaying

    /// Position, on its own observable so only the transport sees the tick.
    let clock = PlaybackClock()
    private(set) var queue: [QueueItem] = []
    /// Queue entries indexed by library track id, so a library row can show
    /// what the queue knows about it — downloading, failed, already played —
    /// without scanning the queue once per row.
    private(set) var queuedByTrack: [Int64: QueueItem] = [:]
    private(set) var devices: [Device] = []
    /// `nil` means system default. Read back from config, so it survives restarts.
    private(set) var currentDevice: String?

    /// Which queue rows are selected.
    ///
    /// Lives on the model rather than in the view because the Edit menu acts on
    /// it, and a menu can't reach a view's `@State`.
    var queueSelection: Set<String> = []

    /// Set while the user drags the seek head, so the poll doesn't yank the
    /// thumb back to the engine's position mid-gesture.
    var scrubbing: Double?
    var lastError: String?

    private var knownQueueVersion: UInt64 = .max
    /// Run after every state read. Now Playing hangs off this rather than
    /// polling the engine a second time on its own timer.
    var onTick: (() -> Void)?

    init(engine: KoanEngine) {
        self.engine = engine
        self.nowPlaying = engine.nowPlaying()
    }

    /// Subscribe to engine events. Nothing here polls any more: the engine
    /// pushes state changes, queue changes and position, so the UI reacts
    /// rather than asking. The watching still happens — it just happens in
    /// Rust, over atomics, off the audio path.
    func start() {
        engine.subscribe(listener: EventBridge(model: self))
        tick()  // Seed from current state; events only carry changes.
        refreshDevices()
    }

    /// Apply a snapshot. Called on subscribe and whenever the engine says
    /// something changed.
    fileprivate func tick() {
        let now = engine.nowPlaying()
        nowPlaying = now
        updateDerived(now)
        // The queue only gets rebuilt when the engine says it changed.
        if nowPlaying.playlistVersion != knownQueueVersion {
            knownQueueVersion = nowPlaying.playlistVersion
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
        let reached = abs(Int64(clock.positionMs) - Int64(target)) < 750
        pendingSeekTicks += 1
        if reached || pendingSeekTicks > 20 {
            pendingSeekMs = nil
            pendingSeekTicks = 0
            scrubbing = nil
        }
    }

    private func rebuildQueue() {
        queue = engine.queue()
        queuedByTrack = Dictionary(
            queue.compactMap { item in item.trackId.map { ($0, item) } },
            // A track queued twice: prefer the entry that is actually doing
            // something over one still sitting idle.
            uniquingKeysWith: { a, b in b.status == .queued ? a : b }
        )
    }

    /// The queue changed — rebuild it and refresh what depends on it.
    fileprivate func applyQueueChange() {
        rebuildQueue()
        tick()
    }

    /// Position moved. The only genuinely periodic event, and the only thing
    /// that should make the transport redraw.
    fileprivate func applyPosition(_ ms: UInt64) {
        clock.update(positionMs: ms, durationMs: clock.durationMs)
        settlePendingSeek()
        onTick?()
    }

    // MARK: - Derived
    //
    // Stored rather than computed, and assigned only on an actual change, so
    // observers of these don't inherit the tick rate of `nowPlaying`.

    private(set) var isPlaying = false
    private(set) var currentTrackId: Int64?
    private(set) var currentItemId: String?
    private(set) var radioEnabled = false
    /// Bumped whenever the engine reports the queue changed. Lets a caller wait
    /// for its own mutation to land.
    private(set) var queueVersion: UInt64 = 0
    /// Queue mutations in flight. Adding a large selection takes a moment, and
    /// silence while it happens reads as nothing having happened.
    private(set) var pendingMutations = 0
    var isBusy: Bool { pendingMutations > 0 }
    /// What is playing, and in what format. Both change per track, not per tick.
    private(set) var currentEntry: QueueItem?
    private(set) var currentFormat: StreamFormat?

    private func updateDerived(_ now: NowPlaying) {
        let playing = now.state == .playing
        if playing != isPlaying { isPlaying = playing }
        if now.entry?.trackId != currentTrackId { currentTrackId = now.entry?.trackId }
        if now.queueItemId != currentItemId { currentItemId = now.queueItemId }
        if now.radioEnabled != radioEnabled { radioEnabled = now.radioEnabled }
        if now.playlistVersion != queueVersion { queueVersion = now.playlistVersion }
        if now.entry?.queueItemId != currentEntry?.queueItemId
            || now.entry?.status != currentEntry?.status
            || now.entry?.downloadProgress != currentEntry?.downloadProgress
        {
            currentEntry = now.entry
        }
        if now.format?.codec != currentFormat?.codec
            || now.format?.sampleRate != currentFormat?.sampleRate
            || now.format?.bitDepth != currentFormat?.bitDepth
        {
            currentFormat = now.format
        }
        clock.update(positionMs: now.positionMs, durationMs: now.durationMs)
    }

    /// 0–1 through the current track. Reflects the drag while scrubbing.
    var progress: Double { scrubbing ?? clock.progress }

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
        seek(toMs: UInt64(max(0, min(1, fraction)) * Double(clock.durationMs)))
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
        let current = Int64(clock.positionMs)
        let target = max(0, current + Int64(delta) * 1000)
        seek(toMs: UInt64(min(target, Int64(clock.durationMs))))
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
        if clock.durationMs > 0 {
            scrubbing = Double(ms) / Double(clock.durationMs)
        }
        attempt { try engine.seek(positionMs: ms) }
    }

    // MARK: - Queue

    /// Replace the queue and start playing — double-clicking an album.
    /// Queue the whole list, starting at `index` — so clicking track nine of an
    /// album still leaves the rest queued behind it.
    ///
    /// The index goes with the command rather than following it as a separate
    /// `play`. Two commands meant the first track started before the cursor
    /// jumped, which showed as track one flashing as playing.
    func playNow(trackIds: [Int64], startingAt index: Int = 0) {
        guard !trackIds.isEmpty else { return }
        let start = trackIds.indices.contains(index) ? index : 0
        offMain { _ = try $0.replaceQueue(trackIds: trackIds, startAt: UInt32(start)) }
    }

    /// Queue immediately after whatever is playing, rather than at the end.
    /// Falls back to appending when nothing is playing to insert after.
    func playNext(trackIds: [Int64]) {
        guard !trackIds.isEmpty else { return }
        guard let cursor = currentItemId else { return enqueue(trackIds: trackIds) }
        offMain { _ = try $0.insertAfter(trackIds: trackIds, afterQueueItemId: cursor) }
    }

    /// Surface a one-off message in the same place engine errors appear.
    func report(_ message: String) { lastError = message }

    func enqueue(trackIds: [Int64]) {
        guard !trackIds.isEmpty else { return }
        offMain { _ = try $0.addToQueue(trackIds: trackIds) }
    }

    func remove(itemIds: [String]) {
        guard !itemIds.isEmpty else { return }
        offMain { try $0.removeFromQueue(queueItemIds: itemIds) }
    }

    /// `after: false` inserts *before* the target, which is the only way to
    /// express "put this at the very top" or "put this above that album".
    func move(itemIds: [String], target: String, after: Bool) {
        offMain {
            try $0.moveInQueue(queueItemIds: itemIds, targetQueueItemId: target, after: after)
        }
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
    func toggleRadio() { setRadio(!radioEnabled) }

    /// Toggles, and returns nothing — the library view refetches to pick it up.
    @discardableResult
    func toggleFavourite(trackId: Int64) -> Bool {
        (try? engine.toggleFavourite(trackId: trackId)) ?? false
    }

    // MARK: - Edit actions
    //
    // Wired to the standard Edit menu, so ⌘A/⌘C/⌘X/⌘V/Delete mean what they
    // mean everywhere else rather than being decorative.

    /// Menus can't write the view's selection directly — the List binds to
    /// local state, and observing the model from the body is what made clicking
    /// unreliable. So this bumps a token the view watches, which only changes
    /// when the command is actually invoked.
    private(set) var selectAllToken = 0

    func selectAllQueue() { selectAllToken += 1 }

    func removeSelected() {
        remove(itemIds: Array(queueSelection))
        queueSelection = []
    }

    /// Track IDs behind the current selection, in queue order.
    var selectedTrackIds: [Int64] {
        queue.filter { queueSelection.contains($0.queueItemId) }.compactMap(\.trackId)
    }

    /// Copies as both a koan payload and plain text: the first lets it be
    /// pasted back into the queue, the second makes it useful anywhere else.
    func copySelection() {
        let items = queue.filter { queueSelection.contains($0.queueItemId) }
        guard !items.isEmpty else { return }
        Pasteboard.write(
            trackIds: items.compactMap(\.trackId),
            text: items.map { "\($0.artist) — \($0.title)" }.joined(separator: "\n")
        )
    }

    func cutSelection() {
        copySelection()
        removeSelected()
    }

    /// Pastes after the selection if there is one, otherwise appends.
    func paste() {
        let ids = Pasteboard.readTrackIds()
        guard !ids.isEmpty else { return }
        if let anchor = queue.last(where: { queueSelection.contains($0.queueItemId) }) {
            offMain { _ = try $0.insertAfter(trackIds: ids, afterQueueItemId: anchor.queueItemId) }
        } else {
            enqueue(trackIds: ids)
        }
    }

    /// Resolve dropped playables and queue them. Order is preserved: dropping a
    /// selection of albums queues them in the order they were dragged.
    func acceptDrop(_ dropped: [PlayableTransfer], playImmediately: Bool = false) {
        guard !dropped.isEmpty else { return }
        let engine = self.engine
        Task {
            let ids = await Task.detached(priority: .userInitiated) {
                dropped.flatMap { $0.trackIds(using: engine) }
            }.value
            guard !ids.isEmpty else { return }
            if playImmediately { playNow(trackIds: ids) } else { enqueue(trackIds: ids) }
        }
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
    ///
    /// For cheap calls only — transport commands are a send down a channel.
    private func attempt(_ body: () throws -> Void) {
        do {
            try body()
        } catch {
            lastError = String(describing: error)
        }
    }

    /// Run a blocking engine call off the main actor.
    ///
    /// Anything that touches the queue resolves every track against the
    /// database and builds a playlist item for each. On an artist that is
    /// thousands of rows, and on the main actor it freezes the window. Nothing
    /// here needs the result, so nothing waits for it — the engine pushes an
    /// event when the queue actually changes.
    private func offMain(_ body: @escaping @Sendable (KoanEngine) throws -> Void) {
        let engine = self.engine
        pendingMutations += 1
        Task {
            do {
                try await Task.detached(priority: .userInitiated) { try body(engine) }.value
            } catch {
                lastError = String(describing: error)
            }
            pendingMutations -= 1
        }
    }
}

/// Bridges the engine's callbacks onto the main actor.
///
/// uniffi calls these from its own thread, so nothing here may touch the model
/// directly — each hop is explicit. Separate from `PlayerModel` because the
/// callback interface must be `Sendable` and the model is main-actor isolated.
final class EventBridge: PlayerEvents, @unchecked Sendable {
    private weak var model: PlayerModel?

    init(model: PlayerModel) {
        self.model = model
    }

    func playbackChanged(nowPlaying: NowPlaying) {
        Task { @MainActor [weak model] in model?.tick() }
    }

    func queueChanged(version: UInt64) {
        Task { @MainActor [weak model] in model?.applyQueueChange() }
    }

    func positionChanged(positionMs: UInt64) {
        Task { @MainActor [weak model] in model?.applyPosition(positionMs) }
    }
}
