import Foundation
import KoanFFI

/// What the app does to the player, and the little it knows that the engine
/// does not.
///
/// Not a copy of engine state — that is `EngineMirror`, and everything read
/// here reads through it. What lives here is the other direction: commands, the
/// spinner and the error banner they need, and the handful of things that are
/// genuinely local because the engine has no opinion about them — which rows
/// are selected, where a thumb is being dragged to.
@MainActor
@Observable
final class PlayerModel {
    let engine: KoanEngine
    @ObservationIgnored let mirror: EngineMirror

    /// Which queue rows are selected.
    ///
    /// Lives on the model rather than in the view because the Edit menu acts on
    /// it, and a menu can't reach a view's `@State`.
    var queueSelection: Set<String> = []

    /// Set while the user drags the seek head, so the engine's own position
    /// doesn't yank the thumb back mid-gesture.
    var scrubbing: Double?
    var lastError: String?
    /// Something the app declined to do, and why. Not a failure — the state it
    /// describes resolves on its own.
    var lastNotice: String?

    private(set) var devices: [Device] = []
    /// `nil` means system default. Read back from config, so it survives restarts.
    private(set) var currentDevice: String?

    /// Queue mutations in flight. Adding a large selection takes a moment, and
    /// silence while it happens reads as nothing having happened.
    private(set) var pendingMutations = 0
    var isBusy: Bool { pendingMutations > 0 }

    /// Set by `AppState`. Queue mutations register here alongside every other
    /// slow thing rather than tracking their own spinner.
    weak var activity: ActivityModel?

    init(engine: KoanEngine, mirror: EngineMirror) {
        self.engine = engine
        self.mirror = mirror
    }

    func start() async {
        refreshDevices()
        startAutosave()
        // The two things that are not views and so have no body to invalidate:
        // where a seek asked to land, and where what is playing lives.
        mirror.follow { [weak self] in self?.followEngine() }
        await reportSignedOutRemote()
    }

    /// Say so when the server is configured but koan has no password for it.
    ///
    /// Nothing else does. The queue fills with tracks that never load, every
    /// sleeve comes back empty and no download starts — which reads as a broken
    /// library rather than as being signed out, and sends people looking in the
    /// wrong place. The one thing that fixes it is a sign-in, so name it.
    private func reportSignedOutRemote() async {
        let settings = await engine.settings()
        guard settings.remoteEnabled, !settings.remoteUrl.isEmpty, !settings.remoteSignedIn
        else { return }
        let host = URL(string: settings.remoteUrl)?.host() ?? settings.remoteUrl
        report("koan has no password for \(host). Sign in again in Settings.")
    }

    /// What the mirror moving means for the two things here that are not
    /// views.
    ///
    /// Reads only the mirror. Nothing it writes is anything it reads — a
    /// follower that observes its own output re-runs until it happens to
    /// settle, and "happens to" is not a property worth relying on.
    private func followEngine() {
        let trackId = mirror.playback.entry?.trackId
        let position = mirror.positionMs
        if trackId != followedTrackId {
            followedTrackId = trackId
            resolveCurrentPlace(trackId: trackId)
        }
        settlePendingSeek(position: position)
    }

    /// The track `followEngine` last acted on. Unobserved on purpose — see there.
    @ObservationIgnored private var followedTrackId: Int64?

    // MARK: - What is playing
    //
    // Read straight through the mirror. Computed rather than stored, so there
    // is one account of each of these and no rule about when to refresh it.

    var isPlaying: Bool { mirror.playback.state == .playing }
    var currentTrackId: Int64? { mirror.playback.entry?.trackId }
    var currentItemId: String? { mirror.playback.queueItemId }
    var currentEntry: QueueItem? { mirror.playback.entry }
    var currentFormat: StreamFormat? { mirror.playback.format }
    var radioEnabled: Bool { mirror.playback.radioEnabled }
    var queueVersion: UInt64 { mirror.playback.playlistVersion }
    var durationMs: UInt64 { mirror.playback.durationMs }
    var queue: [QueueItem] { mirror.queue }

    /// The playlist row that is playing, when what is playing came from one.
    /// A playlist page lights this row and no other — including the other copy
    /// of the same song, which is a different row.
    var currentPlaylistEntryId: Int64? { currentEntry?.playlistEntryId }

    var upNext: [QueueItem] {
        guard let cursor = currentItemId,
              let index = queue.firstIndex(where: { $0.queueItemId == cursor })
        else { return queue }
        return Array(queue.dropFirst(index + 1))
    }

    /// 0–1 through the current track. Reflects the drag while scrubbing.
    var progress: Double {
        if let scrubbing { return scrubbing }
        guard durationMs > 0 else { return 0 }
        return min(1, Double(mirror.positionMs) / Double(durationMs))
    }

    /// Whether the track can be seeked at all yet.
    ///
    /// False while a download is playing that could not say what it is until
    /// the rest of it lands — there is nothing to seek against. It becomes true
    /// on its own when the transfer finishes.
    var canSeek: Bool { mirror.seekableMs > 0 }

    /// How much of the track can be reached, as a fraction.
    ///
    /// 1 for anything on disk. Short of it while a download is still arriving —
    /// the engine clamps a seek to the same extent, so a scrub past this would
    /// land somewhere the thumb was never dragged to.
    var seekable: Double {
        let seekableMs = mirror.seekableMs
        guard durationMs > 0, seekableMs < durationMs else { return 1 }
        return Double(seekableMs) / Double(durationMs)
    }

    /// How much of what is playing has arrived, while it is still arriving.
    ///
    /// Bytes, not reachable time: the two differ for a track that is playing
    /// but cannot be seeked, where the point of the mark is to say the transfer
    /// is going and roughly how far — not to offer a position.
    var fetched: Double? {
        currentItemId.flatMap { mirror.progress(for: $0) }
    }

    // MARK: - Where what is playing lives

    /// The record and the artist behind what is playing, so the transport bar
    /// can link to them.
    ///
    /// A `QueueItem` carries names, not ids — it has to stand for things that
    /// were never in the library. Resolved once when the track changes rather
    /// than per frame: this is a database read.
    private(set) var currentAlbumId: Int64?
    private(set) var currentArtistId: Int64?
    /// The track `currentAlbumId` has actually been resolved for, as opposed to
    /// one still being looked up. Only `currentArtwork` cares, and it cares a
    /// lot — see there.
    private var placeResolvedFor: Int64?

    /// The sleeve to draw for what is playing.
    ///
    /// The record wherever we know it, so every track off one album shares a
    /// single fetch and a single cached bitmap. `nil` while the lookup is still
    /// in flight rather than the track: falling back for those few hundred
    /// milliseconds would fetch the sleeve keyed by track and then fetch the
    /// identical image again keyed by album, which is the duplication this is
    /// here to avoid. A beat of placeholder is cheaper.
    var currentArtwork: AlbumArtwork.Source? {
        if let currentAlbumId { return .album(currentAlbumId) }
        guard placeResolvedFor == currentTrackId else { return nil }
        return currentTrackId.map { .track($0) }
    }

    private func resolveCurrentPlace(trackId: Int64?) {
        guard let trackId else {
            currentAlbumId = nil
            currentArtistId = nil
            placeResolvedFor = nil
            return
        }
        placeResolvedFor = nil
        let engine = self.engine
        Task { @MainActor in
            let track = (try? await engine.track(trackId: trackId)) ?? nil
            // The track moved on while we were asking, so whatever came back
            // belongs to something that is no longer playing.
            guard trackId == self.currentTrackId else { return }
            self.currentAlbumId = track?.albumId
            self.currentArtistId = track?.artistId
            self.placeResolvedFor = trackId
        }
    }

    // MARK: - Transport

    func togglePlayPause() { attempt { try await self.engine.togglePlayPause() } }
    func pause() { attempt { try await self.engine.pause() } }
    func resume() { attempt { try await self.engine.resume() } }
    func next() { attempt { try await self.engine.next() } }
    func previous() { attempt { try await self.engine.previous() } }
    func stop() { attempt { try await self.engine.stop() } }

    func play(itemId: String) { attempt { try await self.engine.play(queueItemId: itemId) } }

    /// Where a seek asked to land, until the engine reports being near it.
    @ObservationIgnored private var pendingSeekMs: UInt64?
    @ObservationIgnored private var pendingSeekTicks = 0

    /// Release the held position once the engine has caught up — or give up, so
    /// a seek the engine rejected can't wedge the bar permanently.
    private func settlePendingSeek(position: UInt64) {
        guard let target = pendingSeekMs else { return }
        let reached = abs(Int64(position) - Int64(target)) < 750
        pendingSeekTicks += 1
        if reached || pendingSeekTicks > 20 {
            pendingSeekMs = nil
            pendingSeekTicks = 0
            scrubbing = nil
        }
    }

    /// Commit a scrub. Position comes from the drag, not the engine, and stops
    /// at what has been downloaded.
    func seek(fraction: Double) {
        seek(toMs: UInt64(clamp(fraction) * Double(durationMs)))
    }

    /// Called as the thumb is dragged. Cancels any seek still settling, since
    /// the user is now the authority on where the head is.
    func beginScrub(fraction: Double) {
        pendingSeekMs = nil
        pendingSeekTicks = 0
        scrubbing = clamp(fraction)
    }

    /// A drag position held inside the track and inside what has arrived.
    private func clamp(_ fraction: Double) -> Double {
        min(seekable, min(1, max(0, fraction)))
    }

    /// Why the playhead did not move. Reaching for a position in a track that
    /// has not arrived is a reasonable thing to try, and a bar that simply
    /// ignores the attempt teaches nothing.
    func explainUnseekable() {
        let progress = fetched.map { " — \(Int($0 * 100))% so far" } ?? ""
        lastNotice = "Still downloading\(progress). This track can be seeked once it has finished."
    }

    /// Nudge by a number of seconds, clamped to the track. What the arrow-key
    /// shortcuts and the TUI's `,`/`.` do.
    func seek(bySeconds delta: Int) {
        guard canSeek else { return explainUnseekable() }
        let current = Int64(mirror.positionMs)
        let target = max(0, current + Int64(delta) * 1000)
        seek(toMs: UInt64(min(target, Int64(mirror.seekableMs))))
    }

    /// Hold the requested position until the engine agrees with it.
    ///
    /// The seek is asynchronous — it goes down a channel to the player thread,
    /// which restarts decoding before `position_ms` moves. Clearing the local
    /// value when the command is merely *sent* hands the bar back to the engine
    /// during that gap, so the bar reads the old position and the thumb snaps
    /// backwards before jumping forward again.
    func seek(toMs ms: UInt64) {
        pendingSeekMs = ms
        if durationMs > 0 {
            scrubbing = Double(ms) / Double(durationMs)
        }
        attempt { try await self.engine.seek(positionMs: ms) }
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
        mutate { _ = try await $0.replaceQueue(trackIds: trackIds, startAt: UInt32(start)) }
    }

    /// Queue immediately after whatever is playing, rather than at the end.
    /// Falls back to appending when nothing is playing to insert after.
    func playNext(trackIds: [Int64]) {
        guard !trackIds.isEmpty else { return }
        guard let cursor = currentItemId else { return enqueue(trackIds: trackIds) }
        mutate { _ = try await $0.insertAfter(trackIds: trackIds, afterQueueItemId: cursor) }
    }

    /// Surface a one-off message in the same place engine errors appear.
    func report(_ message: String) { lastError = message }

    func enqueue(trackIds: [Int64]) {
        guard !trackIds.isEmpty else { return }
        mutate { _ = try await $0.addToQueue(trackIds: trackIds) }
    }

    func remove(itemIds: [String]) {
        guard !itemIds.isEmpty else { return }
        mutate { try await $0.removeFromQueue(queueItemIds: itemIds) }
    }

    /// `after: false` inserts *before* the target, which is the only way to
    /// express "put this at the very top" or "put this above that album".
    func move(itemIds: [String], target: String, after: Bool) {
        mutate {
            try await $0.moveInQueue(queueItemIds: itemIds, targetQueueItemId: target, after: after)
        }
    }

    func clearQueue() { attempt { try await self.engine.clearQueue() } }
    func undo() { attempt { try await self.engine.undo() } }
    func redo() { attempt { try await self.engine.redo() } }

    // MARK: - Devices & modes

    func refreshDevices() {
        let engine = self.engine
        Task {
            let found = (try? await engine.devices()) ?? []
            self.devices = found
            self.currentDevice = await engine.currentDevice()
        }
    }

    func setDevice(_ name: String?) {
        attempt {
            if let name { try await self.engine.setDevice(name: name) } else { try await self.engine.clearDevice() }
        }
        currentDevice = name
    }

    func setRadio(_ enabled: Bool) { engine.setRadio(enabled: enabled) }

    /// Flips it without the caller having to read the current value — menus
    /// that read observable state rebuild themselves constantly.
    func toggleRadio() { setRadio(!radioEnabled) }

    // MARK: - Edit actions
    //
    // Wired to the standard Edit menu, so ⌘A/⌘C/⌘X/⌘V/Delete mean what they
    // mean everywhere else rather than being decorative.

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
            mutate { _ = try await $0.insertAfter(trackIds: ids, afterQueueItemId: anchor.queueItemId) }
        } else {
            enqueue(trackIds: ids)
        }
    }

    /// Index files dropped from Finder into the library, then queue them.
    ///
    /// They are indexed where they lie rather than copied anywhere: a drop is
    /// "play this", and giving the files library rows is what lets organize
    /// move them into the music tree afterwards, on purpose and with a preview.
    /// Folders are walked, so dropping a rip queues the album.
    func importFiles(_ urls: [URL]) {
        let paths = urls.filter(\.isFileURL).map(\.path)
        guard !paths.isEmpty else { return }
        let engine = self.engine
        Task {
            // Holds the local library: it reads tags and writes rows, the same
            // ones a scan would. A folder of a few hundred files takes long
            // enough that a drop with no sign of life reads as a drop that
            // missed.
            let summary = try? await activity?.run("Adding dropped files", uses: .localLibrary) {
                try await engine.importFiles(paths: paths)
            }.get()
            guard let summary, !summary.trackIds.isEmpty else {
                lastError = "Nothing there koan can play."
                return
            }
            if let first = summary.errors.first {
                lastError = first
            }
            enqueue(trackIds: summary.trackIds)
        }
    }

    /// Resolve dropped playables and queue them. Order is preserved: dropping a
    /// selection of albums queues them in the order they were dragged.
    func acceptDrop(_ dropped: [PlayableTransfer], playImmediately: Bool = false) {
        guard !dropped.isEmpty else { return }
        let engine = self.engine
        Task {
            var ids: [Int64] = []
            for item in dropped {
                ids += await item.trackIds(using: engine)
            }
            guard !ids.isEmpty else { return }
            if playImmediately { playNow(trackIds: ids) } else { enqueue(trackIds: ids) }
        }
    }

    // MARK: - Session

    /// The queue version last written whole, so the blob is only rewritten when
    /// the queue is what changed.
    @ObservationIgnored private var savedQueueVersion: UInt64 = 0

    /// Persist the queue and position. Called on quit, and periodically so an
    /// unclean exit doesn't lose the session.
    func saveSession() async {
        try? await engine.saveSession()
        savedQueueVersion = queueVersion
    }

    /// Persist often enough that a crash costs a second, not the session.
    ///
    /// Position goes every second and is four columns; the queue is a JSON blob
    /// and only rewritten when it actually changes, because re-serialising a
    /// library-sized queue once a second would be megabytes of writing to
    /// remember one number.
    private func startAutosave() {
        Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(1))
                guard let self else { return }
                if self.queueVersion != self.savedQueueVersion {
                    await self.saveSession()
                } else if self.currentEntry != nil {
                    try? await self.engine.savePosition()
                }
            }
        }
    }

    /// Restore the queue from the last session without starting playback.
    ///
    /// Waited on rather than fired off: the first frame of the queue should be
    /// the queue you left, not an empty list that fills in behind the window.
    /// The engine says how many rows it restored and the player thread applies
    /// them, so this holds until the mirror shows them — and never for long,
    /// because a restore slow enough to notice is not a reason to keep the
    /// window shut.
    func restoreSession() async {
        guard let restored = try? await engine.restoreSession(), restored > 0 else { return }
        let deadline = ContinuousClock.now + .milliseconds(500)
        while mirror.queue.count < Int(restored), ContinuousClock.now < deadline {
            try? await Task.sleep(for: .milliseconds(5))
        }
    }

    // MARK: - Errors

    /// Engine calls fail for real reasons (device vanished, track gone) but
    /// none of them are worth a modal. Surface it and carry on.
    private func attempt(_ body: @escaping () async throws -> Void) {
        Task {
            do {
                try await body()
            } catch {
                lastError = String(describing: error)
            }
        }
    }

    /// A queue mutation, with the spinner and the error reporting every one of
    /// them wants.
    ///
    /// Nothing waits for the result — the engine publishes the queue when it
    /// actually changes — but these are ordered against each other on the
    /// engine's side, so dropping in an album and then pressing undo cannot
    /// land the wrong way round.
    private func mutate(_ body: @escaping (KoanEngine) async throws -> Void) {
        let engine = self.engine
        pendingMutations += 1
        let job = activity?.begin("Updating queue")
        Task {
            do {
                try await body(engine)
            } catch {
                lastError = String(describing: error)
            }
            if let job { activity?.end(job) }
            pendingMutations -= 1
        }
    }
}
