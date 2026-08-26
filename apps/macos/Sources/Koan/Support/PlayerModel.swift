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
    /// How far into the current track a seek can land. Equal to the duration
    /// for anything on disk, and short of it while a download is still
    /// arriving. Observed, unlike the snapshot it comes from, because the seek
    /// bar draws it — and change-guarded, because it moves while a track caches
    /// and then stops for the rest of the record.
    private(set) var seekableMs: UInt64 = 0
    private(set) var queue: [QueueItem] = []
    /// Queue entries indexed by library track id, so a library row can show
    /// what the queue knows about it — downloading, failed, already played —
    /// without scanning the queue once per row.
    private(set) var queuedByTrack: [Int64: QueueItem] = [:]
    /// Queue entries indexed by the playlist row they came from.
    ///
    /// Keyed on the entry rather than the track, because a playlist may hold
    /// the same track twice and `queuedByTrack` can only answer for one of
    /// them. This is what lets a playlist row ask about *itself*.
    private(set) var queuedByPlaylistEntry: [Int64: QueueItem] = [:]
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
    /// Something the app declined to do, and why. Not a failure — the state it
    /// describes resolves on its own.
    var lastNotice: String?

    private var knownQueueVersion: UInt64 = .max
    /// Run after every state read. Now Playing hangs off this rather than
    /// polling the engine a second time on its own timer.
    var onTick: (() -> Void)?

    /// The loop following the engine. Cancelling it ends the subscription.
    @ObservationIgnored private var events: Task<Void, Never>?

    init(engine: KoanEngine) {
        self.engine = engine
        // Reading the engine is a suspension now, so the first frame renders
        // against a stopped transport and `start()` fills it in.
        self.nowPlaying = NowPlaying(
            state: .stopped, positionMs: 0, durationMs: 0, seekableMs: 0,
            queueItemId: nil, entry: nil, format: nil, playlistVersion: 0,
            radioEnabled: false
        )
    }

    /// Subscribe to engine events. Nothing here polls any more: the engine
    /// pushes state changes, queue changes and position, so the UI reacts
    /// rather than asking. The watching still happens — it just happens in
    /// Rust, over atomics, off the audio path.
    func start() async {
        await tick()  // Seed from current state; events only carry changes.
        refreshDevices()
        startAutosave()
        observe()
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

    /// Follow the engine for as long as this model is alive.
    ///
    /// The loop *is* the subscription — there is nothing to register and
    /// nothing to unregister, and cancelling the task ends it. Weakly held so
    /// a model that goes away takes its loop with it.
    private func observe() {
        events?.cancel()
        events = Task { [weak self] in
            guard let engine = self?.engine else { return }
            for await event in engine.events() {
                guard let self else { return }
                switch event {
                case .playbackChanged(let nowPlaying):
                    self.apply(nowPlaying)
                case .queueChanged:
                    self.applyQueueChange()
                case .positionChanged(let positionMs):
                    self.applyPosition(positionMs)
                case .downloadsChanged(let downloads):
                    self.applyDownloads(downloads)
                    self.onDownloadProgress?(downloads)
                case .downloadStoreChanged:
                    self.onDownloadStoreChanged?()
                case .libraryChanged:
                    self.onLibraryChanged?()
                }
            }
        }
    }

    /// Seed from the engine. Only the initial subscribe needs this — every
    /// event that reports a change carries the new state with it.
    fileprivate func tick() async {
        apply(await engine.nowPlaying())
    }

    /// Apply a snapshot the engine sent.
    ///
    /// Nothing here waits. This runs on the thread draining the event stream,
    /// and that stream is sequential: an `await` on a database round trip here
    /// held up every message behind it — position included — for as long as it
    /// took. Which is why playback appeared to stall precisely when downloads
    /// were busy bumping the queue version.
    fileprivate func apply(_ now: NowPlaying) {
        nowPlaying = now
        updateDerived(now)
        // The queue only gets rebuilt when the engine says it changed, and the
        // rebuild happens somewhere this loop is not.
        if now.playlistVersion != knownQueueVersion {
            knownQueueVersion = now.playlistVersion
            scheduleQueueRebuild()
        }
        settlePendingSeek()
        onTick?()
    }

    /// The rebuild in flight, if there is one.
    @ObservationIgnored private var queueRebuild: Task<Void, Never>?
    /// Whether the queue changed again while one was running.
    @ObservationIgnored private var queueRebuildPending = false

    /// Rebuild the queue off the event stream, and only once for any number of
    /// changes that arrive while one is running.
    ///
    /// A download changing state bumps the queue version, so a handful of
    /// transfers announce themselves several times a second. Rebuilding per
    /// announcement meant a database round trip per announcement, in front of
    /// every other event.
    private func scheduleQueueRebuild() {
        guard queueRebuild == nil else {
            queueRebuildPending = true
            return
        }
        queueRebuild = Task { @MainActor [weak self] in
            defer { self?.queueRebuild = nil }
            repeat {
                self?.queueRebuildPending = false
                await self?.rebuildQueue()
            } while self?.queueRebuildPending == true
        }
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

    private func rebuildQueue() async {
        queue = await engine.queue()
        // Here rather than at one of the callers: the queue is rebuilt from
        // two places — a queue event, and a playback event carrying a version
        // that moved — and only one of them used to say so. Which of the two
        // arrived last decided whether anything watching the queue heard about
        // it, so a playlist would light up as locked sometimes and not others.
        defer { onQueueChanged?() }
        queuedByTrack = Dictionary(
            queue.compactMap { item in item.trackId.map { ($0, item) } },
            // A track queued twice: prefer the entry that is actually doing
            // something over one still sitting idle.
            uniquingKeysWith: { a, b in b.status == .queued ? a : b }
        )
        queuedByPlaylistEntry = Dictionary(
            queue.compactMap { item in item.playlistEntryId.map { ($0, item) } },
            uniquingKeysWith: { a, b in b.status == .queued ? a : b }
        )
    }

    /// Queue items mid-transfer at the last report.
    ///
    /// A track leaving this set has landed on disk, which is the only thing
    /// that moves the library's cached count — and nothing else would tell it.
    private var downloading: Set<String> = []

    /// Download progress moved.
    ///
    /// Patched into the queue in place rather than refetching it. This arrives
    /// several times a second while the queue behind it has not changed, and
    /// rebuilding the whole list for a byte counter is what made the rest of
    /// the app stutter while an album cached.
    fileprivate func applyDownloads(_ downloads: [DownloadProgress]) {
        let byItem = Dictionary(
            downloads.map { ($0.queueItemId, $0.progress) },
            uniquingKeysWith: { first, _ in first }
        )
        for index in queue.indices {
            let progress = byItem[queue[index].queueItemId] ?? nil
            guard queue[index].downloadProgress != progress else { continue }
            queue[index].downloadProgress = progress
            if let trackId = queue[index].trackId {
                queuedByTrack[trackId] = queue[index]
            }
        }

        // The transport reads `currentEntry`, which is its own copy and not one
        // of the rows above. Without this the seek bar's downloaded extent only
        // moved when the state or the cursor did — so a track that spends its
        // whole download unable to say what it is showed a figure frozen at
        // whatever had arrived when it started, or nothing at all.
        if let entry = currentEntry {
            let progress = byItem[entry.queueItemId] ?? nil
            if entry.downloadProgress != progress {
                currentEntry?.downloadProgress = progress
            }
        }

        let active = Set(byItem.keys)
        if !downloading.subtracting(active).isEmpty {
            onDownloadsLanded?()
        }
        downloading = active
    }

    /// The queue changed. Set by `AppState` — whether the queue is still
    /// exactly some playlist is a question only the engine can answer, and this
    /// is the moment the answer can have changed.
    var onQueueChanged: (() -> Void)?

    /// A download finished. Set by `AppState` — the library's cached count is
    /// the only thing that knows it moved, and it has no other way to find out.
    var onDownloadsLanded: (() -> Void)?

    /// The library's rows changed. Set by `AppState`. This is how a scan or a
    /// sync nobody asked for reaches the browser — the engine says so rather
    /// than the app having to guess from whatever it happened to start itself.
    var onLibraryChanged: (() -> Void)?
    /// The set of transfers changed shape — one appeared or settled.
    var onDownloadStoreChanged: (() -> Void)?
    /// Byte counts moved. Fires several times a second while anything is
    /// downloading, and not at all the rest of the time.
    var onDownloadProgress: (([DownloadProgress]) -> Void)?

    /// The queue changed — rebuild it and refresh what depends on it.
    fileprivate func applyQueueChange() {
        scheduleQueueRebuild()
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
    /// The queue version last written whole, so the blob is only rewritten when
    /// the queue is what changed.
    private var savedQueueVersion: UInt64 = 0
    /// Queue mutations in flight. Adding a large selection takes a moment, and
    /// silence while it happens reads as nothing having happened.
    /// Set by `AppState`. Queue mutations register here alongside every other
    /// slow thing rather than tracking their own spinner.
    weak var activity: ActivityModel?

    private(set) var pendingMutations = 0
    var isBusy: Bool { pendingMutations > 0 }
    /// What is playing, and in what format. Both change per track, not per
    /// tick — except the output rate, which any other client of the device can
    /// move mid-track.
    private(set) var currentEntry: QueueItem?
    /// The playlist row that is playing, when what is playing came from one.
    /// A playlist page lights this row and no other — including the other copy
    /// of the same song, which is a different row.
    var currentPlaylistEntryId: Int64? { currentEntry?.playlistEntryId }
    private(set) var currentFormat: StreamFormat?

    /// Where what is playing lives, so the transport bar can link to it.
    ///
    /// A `QueueItem` carries names, not ids — it has to stand for things that
    /// were never in the library. Resolved once when the track changes rather
    /// than per frame: the transport polls at 10 Hz and this is a database
    /// read.
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

    private func updateDerived(_ now: NowPlaying) {
        let playing = now.state == .playing
        if playing != isPlaying { isPlaying = playing }
        if now.entry?.trackId != currentTrackId {
            currentTrackId = now.entry?.trackId
            resolveCurrentPlace(trackId: currentTrackId)
        }
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
            || now.format?.outputSampleRate != currentFormat?.outputSampleRate
        {
            currentFormat = now.format
        }
        if now.seekableMs != seekableMs { seekableMs = now.seekableMs }
        clock.update(positionMs: now.positionMs, durationMs: now.durationMs)
    }

    /// 0–1 through the current track. Reflects the drag while scrubbing.
    var progress: Double { scrubbing ?? clock.progress }

    /// Whether the track can be seeked at all yet.
    ///
    /// False while a download is playing that could not say what it is until
    /// the rest of it lands — there is nothing to seek against. It becomes true
    /// on its own when the transfer finishes.
    var canSeek: Bool { seekableMs > 0 }

    /// How much of the track can be reached, as a fraction.
    ///
    /// 1 for anything on disk. Short of it while a download is still arriving —
    /// the engine clamps a seek to the same extent, so a scrub past this would
    /// land somewhere the thumb was never dragged to.
    var seekable: Double {
        guard clock.durationMs > 0, seekableMs < clock.durationMs else { return 1 }
        return Double(seekableMs) / Double(clock.durationMs)
    }

    /// How much of what is playing has arrived, while it is still arriving.
    ///
    /// Bytes, not reachable time: the two differ for a track that is playing
    /// but cannot be seeked, where the point of the mark is to say the transfer
    /// is going and roughly how far — not to offer a position.
    var fetched: Double? { currentEntry?.downloadProgress }

    var upNext: [QueueItem] {
        guard let cursor = nowPlaying.queueItemId,
              let index = queue.firstIndex(where: { $0.queueItemId == cursor })
        else { return queue }
        return Array(queue.dropFirst(index + 1))
    }

    // MARK: - Transport

    func togglePlayPause() { attempt { try await self.engine.togglePlayPause() } }
    func pause() { attempt { try await self.engine.pause() } }
    func resume() { attempt { try await self.engine.resume() } }
    func next() { attempt { try await self.engine.next() } }
    func previous() { attempt { try await self.engine.previous() } }
    func stop() { attempt { try await self.engine.stop() } }

    func play(itemId: String) { attempt { try await self.engine.play(queueItemId: itemId) } }

    /// Commit a scrub. Position comes from the drag, not the engine, and stops
    /// at what has been downloaded.
    func seek(fraction: Double) {
        seek(toMs: UInt64(clamp(fraction) * Double(clock.durationMs)))
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
        let current = Int64(clock.positionMs)
        let target = max(0, current + Int64(delta) * 1000)
        seek(toMs: UInt64(min(target, Int64(seekableMs))))
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

    /// Reflected locally as well as set on the engine. The engine emits events
    /// for playback, queue and position — nothing for radio — so a UI driven by
    /// those events would never learn the flag had changed, and the toggle read
    /// as dead.
    func setRadio(_ enabled: Bool) {
        engine.setRadio(enabled: enabled)
        radioEnabled = enabled
    }

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
    ///
    /// `indexed` fires once rows exist, so the library browser can pick up the
    /// artists and albums the drop just created. The player can't do that
    /// itself — it doesn't own the browse caches.
    func importFiles(_ urls: [URL]) {
        let paths = urls.filter(\.isFileURL).map(\.path)
        guard !paths.isEmpty else { return }
        let engine = self.engine
        Task {
            // Exclusive: it reads tags and writes rows, so it queues behind the
            // same SQLite writer a scan does. A folder of a few hundred files
            // takes long enough that a drop with no sign of life reads as a
            // drop that missed.
            let summary = try? await activity?.run("Adding dropped files", exclusive: true) {
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
                } else if self.nowPlaying.entry != nil {
                    try? await self.engine.savePosition()
                }
            }
        }
    }

    /// Restore the queue from the last session without starting playback.
    func restoreSession() {
        let engine = self.engine
        Task {
            _ = try? await engine.restoreSession()
        }
    }

    // MARK: - Errors

    /// Engine calls fail for real reasons (device vanished, track gone) but
    /// none of them are worth a modal. Surface it and carry on.
    ///
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
    /// Nothing waits for the result — the engine pushes an event when the queue
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

/// Pulls events from the engine as an `AsyncSequence`.
///
/// `nextEvent()` answers one at a time and returns nil once the engine is
/// gone; this makes that a `for await` loop, so a client's subscription lives
/// exactly as long as the task reading it.
extension KoanEngine {
    func events() -> AsyncStream<PlayerEvent> {
        AsyncStream { continuation in
            let pump = Task {
                while let event = await self.nextEvent() {
                    continuation.yield(event)
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in pump.cancel() }
        }
    }
}
