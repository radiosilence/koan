#if os(macOS)
import CoreAudio
#endif
import Foundation
import KoanFFI

/// What the app does to the player, and the little it knows that the engine
/// does not.
///
/// Not a copy of engine state — that is `EngineMirror`, and everything read
/// here reads through it. What lives here is the other direction: commands, the
/// spinner and the error banner they need, and the handful of things that are
/// local because the engine has no opinion about them — which rows
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
    /// A share link made on a device with no pasteboard, waiting to be shown
    /// as a code a phone can scan.
    var sharedLink: String?

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

    @ObservationIgnored private var terminating: NSObjectProtocol?

    init(engine: KoanEngine, mirror: EngineMirror) {
        self.engine = engine
        self.mirror = mirror
    }

    func start() async {
        refreshDevices()
        followDevices()
        followSession()
        // A renderer plays from a URL this process serves, which goes with
        // it: stop it rather than leave it to play out and stall. The session
        // was saved as playing by the autosave, so the next launch resumes
        // there.
        terminating = NotificationCenter.default.addObserver(
            forName: .appTerminates, object: nil, queue: .main
        ) { [engine] _ in
            engine.releaseOutput()
        }
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
        report("kōan has no password for \(host). Sign in again in Settings.")
    }

    /// What the mirror moving means for the two things here that are not
    /// views.
    ///
    /// Reads only the mirror. Nothing it writes is anything it reads — a
    /// follower that observes its own output re-runs until it happens to
    /// settle, and "happens to" is not a property worth relying on.
    private func followEngine() {
        let trackId = mirror.playback.entry?.trackId
        let position = mirror.playhead.at()
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
    var shuffle: Bool { mirror.playback.shuffle }
    var repeatMode: RepeatMode { mirror.playback.repeatMode }
    var sleep: SleepState? { mirror.playback.sleep }
    /// Asked to play a track that has not arrived yet, which with nothing on
    /// screen to say so reads as a tap that did nothing. A wait paused by hand
    /// reads as paused, since it will open paused.
    var isWaitingForTrack: Bool {
        mirror.playback.waiting && mirror.playback.state == .stopped
    }
    var currentTrackId: Int64? { mirror.playback.entry?.trackId }
    var currentItemId: String? { mirror.playback.queueItemId }
    var currentEntry: QueueItem? { mirror.playback.entry }
    var currentFormat: StreamFormat? { mirror.playback.format }
    var queueVersion: UInt64 { mirror.queueVersion }
    var durationMs: UInt64 { mirror.playback.durationMs }
    var queue: [QueueItem] { mirror.queue }

    /// The playlist row that is playing, when what is playing came from one.
    /// A playlist page lights this row and no other — including the other copy
    /// of the same song, which is a different row.
    var currentPlaylistEntryId: Int64? { currentEntry?.playlistEntryId }

    /// Where the playhead was last said to be, and whether it is still
    /// moving. What draws a position derives it from this rather than being
    /// handed a number ten times a second — see `Playhead`.
    var playhead: Playhead { mirror.playhead }

    /// 0–1 through the current track, as of now. Reflects the drag while
    /// scrubbing. A value for a moment, not a value that arrives: anything
    /// drawing it continuously should animate toward the end of the track
    /// instead of asking again.
    var progress: Double {
        if let scrubbing { return scrubbing }
        guard durationMs > 0 else { return 0 }
        return min(1, Double(mirror.playhead.at(within: durationMs)) / Double(durationMs))
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

    // MARK: - Where what is playing lives

    /// The record and the artist behind what is playing, so the transport bar
    /// can link to them.
    ///
    /// A `QueueItem` carries names, not ids — it has to stand for things that
    /// were never in the library. Resolved once when the track changes rather
    /// than per frame: this is a database read.
    private(set) var currentAlbumId: Int64?
    private(set) var currentArtistId: Int64?
    /// The track `currentAlbumId` has been resolved for, as opposed to
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
        Task {
            let track = (try? await engine.track(trackId: trackId)) ?? nil
            // The track moved on while we were asking, so whatever came back
            // belongs to something that is no longer playing.
            guard trackId == self.currentTrackId else { return }
            // Written only where the answer moved. `@Observable` has no
            // opinion about equality, so the next track off the same record
            // would otherwise re-run everything coloured by it.
            if self.currentAlbumId != track?.albumId { self.currentAlbumId = track?.albumId }
            if self.currentArtistId != track?.artistId { self.currentArtistId = track?.artistId }
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

    func setShuffle(_ on: Bool) { attempt { try await self.engine.setShuffle(on: on) } }
    func setRepeat(_ mode: RepeatMode) { attempt { try await self.engine.setRepeat(mode: mode) } }
    func toggleShuffle() { setShuffle(!shuffle) }
    /// Off, the queue, one, and round again — the one button's steps.
    func cycleRepeat() {
        let next: RepeatMode = switch repeatMode {
        case .off: .queue
        case .queue: .one
        case .one: .off
        }
        setRepeat(next)
    }

    func setSleepTimer(_ timer: SleepTimer) { attempt { try await self.engine.setSleepTimer(timer: timer) } }
    func cancelSleepTimer() { attempt { try await self.engine.cancelSleepTimer() } }

    func play(itemId: String) { attempt { try await self.engine.play(queueItemId: itemId) } }

    /// Where a seek asked to land, until the engine reports being near it.
    @ObservationIgnored private var pendingSeekMs: UInt64?
    /// When that seek gives up waiting.
    @ObservationIgnored private var pendingSeekDeadline: ContinuousClock.Instant?

    /// Close enough to the target to count as having landed.
    private static let seekTolerance: Int64 = 750
    /// How long a seek waits for the engine before handing the bar back.
    private static let seekPatience: Duration = .seconds(2)

    /// Release the held position once the engine has caught up — or give up, so
    /// a seek the engine rejected can't wedge the bar permanently.
    private func settlePendingSeek(position: UInt64) {
        guard let target = pendingSeekMs else { return }
        let reached = abs(Int64(position) - Int64(target)) < Self.seekTolerance
        let expired = pendingSeekDeadline.map { .now >= $0 } ?? true
        if reached || expired {
            pendingSeekMs = nil
            pendingSeekDeadline = nil
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
        pendingSeekDeadline = nil
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
        let fetched = currentTrackId.flatMap { mirror.figure(for: $0)?.progress }
        let progress = fetched.map { " — \(Int($0 * 100))% so far" } ?? ""
        lastNotice = "Still downloading\(progress). This track can be seeked once it has finished."
    }

    /// Nudge by a number of seconds, clamped to the track. What the arrow-key
    /// shortcuts and the TUI's `,`/`.` do.
    func seek(bySeconds delta: Int) {
        guard canSeek else { return explainUnseekable() }
        let current = Int64(mirror.playhead.at())
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
    func seek(toMs requested: UInt64) {
        guard canSeek else { return explainUnseekable() }
        let ms = min(requested, mirror.seekableMs)
        let deadline = ContinuousClock.now + Self.seekPatience
        pendingSeekMs = ms
        pendingSeekDeadline = deadline
        if durationMs > 0 {
            scrubbing = Double(ms) / Double(durationMs)
        }
        attempt { try await self.engine.seek(positionMs: ms) }
        // The playhead may never move again to settle a rejected seek.
        Task { [weak self] in
            try? await Task.sleep(until: deadline, clock: .continuous)
            guard let self, self.pendingSeekDeadline == deadline else { return }
            self.settlePendingSeek(position: self.mirror.playhead.at())
        }
    }

    // MARK: - Queue

    /// Replace the queue with the whole list and start playing at `index` — so
    /// clicking track nine of an album still leaves the rest queued behind it.
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
    /// The last edit to a playlist taken back, or to the queue when none is
    /// named.
    func undo(playlist: Int64? = nil) { attempt { try await self.engine.undo(playlistId: playlist) } }
    func redo(playlist: Int64? = nil) { attempt { try await self.engine.redo(playlistId: playlist) } }

    // MARK: - Devices & modes

    /// The latest device read. CoreAudio posts several changes for one plug,
    /// and the reads are offloaded, so an older one can finish last.
    @ObservationIgnored private var deviceRead = 0

    func refreshDevices() {
        let engine = self.engine
        deviceRead += 1
        let read = deviceRead
        Task {
            // The Output menu reads the engine's list, which follows this.
            await engine.refreshOutputs()
            let found = (try? await engine.devices()) ?? []
            let current = await engine.currentDevice()
            guard read == self.deviceRead else { return }
            self.devices = found
            self.currentDevice = current
        }
    }

    /// Re-reads the outputs whenever CoreAudio's list of devices changes, so a
    /// DAC plugged in after launch can be picked without a restart.
    private func followDevices() {
        #if os(macOS)
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioHardwarePropertyDevices,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        AudioObjectAddPropertyListenerBlock(
            AudioObjectID(kAudioObjectSystemObject), &address, .main
        ) { [weak self] _, _ in
            MainActor.assumeIsolated { self?.refreshDevices() }
        }

        // The system's output moved: an AirPlay speaker picked from the
        // AirPlay button, headphones plugged in. Playing to the system
        // default, the music follows it, where it was. A device picked by
        // name, a renderer, or another kōan being controlled keep theirs.
        var defaultOutput = AudioObjectPropertyAddress(
            mSelector: kAudioHardwarePropertyDefaultOutputDevice,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        AudioObjectAddPropertyListenerBlock(
            AudioObjectID(kAudioObjectSystemObject), &defaultOutput, .main
        ) { [weak self] _, _ in
            MainActor.assumeIsolated {
                guard let self, self.currentDevice == nil, self.renderer == nil,
                      !self.isControllingAnother
                else { return }
                self.attempt { try await self.engine.restartOutput() }
            }
        }
        #endif
    }

    func setDevice(_ name: String?) {
        attempt {
            if let name { try await self.engine.setDevice(name: name) } else { try await self.engine.clearDevice() }
        }
        currentDevice = name
    }

    // MARK: - Where music plays

    /// The other device being controlled; `nil` while it is this one.
    var controlled: DeviceInfo? {
        guard let target = mirror.target else { return nil }
        return mirror.devices.first { $0.id == target }
    }

    /// Any other device to play on is known of.
    /// Or iOS is hiding them: the picker is where that is explained.
    var hasOtherDevices: Bool {
        !mirror.devices.isEmpty || mirror.connection?.localNetworkBlocked == true
    }

    /// Controlling another device, whether or not it is still listed.
    var isControllingAnother: Bool { mirror.target != nil }

    /// Show and command `id`, or this device with `nil`. Nothing moves.
    func control(_ id: String?) {
        attempt { try await self.engine.controlDevice(id: id) }
    }

    /// Drop a device out of reach from the list, until it is heard from
    /// again. One of the account's is forgotten by the server too.
    func forget(_ id: String) {
        attempt { try await self.engine.forgetDevice(id: id) }
    }

    /// The renderer playing this device's music, if one is.
    var renderer: RendererOutput? { mirror.rendererOutput }

    /// What the device in view plays through: this one, or the one being
    /// controlled.
    var outputs: OutputsInfo? { mirror.outputs }

    /// Whether the device in view's output can be chosen from here: this
    /// device's always, another's if it is the account's own or lets it.
    var canChooseOutput: Bool {
        // Another account's device, shared or on this network, publishes its
        // outputs only when it lets them be chosen.
        !isControllingAnother || controlled?.account == true || mirror.outputs != nil
    }

    /// Play the device in view through `output`. On another device it
    /// switches as its own menu would; the music carries on where it is.
    func selectOutput(_ output: OutputChoice) {
        if outputs?.current == output { return }
        attempt { try await self.engine.selectOutput(output: output) }
        if !isControllingAnother {
            switch output {
            case .device(let name): currentDevice = name
            case .default: currentDevice = nil
            case .renderer: break
            }
        }
    }

    /// The volume of the renderer the device in view plays to.
    func setOutputVolume(_ volume: UInt8) {
        attempt { try await self.engine.setOutputVolume(volume: volume) }
    }

    /// Play `device`, an output of the device in view, through `profile`.
    func setOutputPreset(device: String, profile: String?) {
        attempt { try await self.engine.setOutputPreset(device: device, profile: profile) }
    }

    /// Play to the renderer `udn` in place of this device's output, or back
    /// here with `nil`. The music carries on from where it is.
    func playOn(renderer udn: String?) {
        attempt { try await self.engine.playToRenderer(udn: udn) }
    }

    func setRendererVolume(_ volume: UInt8) {
        attempt { try await self.engine.setRendererVolume(volume: volume) }
    }

    /// Look for renderers on the network; they arrive over a second or two.
    func searchRenderers() {
        engine.searchRenderers()
    }

    /// Send what the controlled device is playing to `id` (this device with
    /// `nil`), and control it there.
    func moveMusic(to id: String?) {
        attempt {
            let left = try await self.engine.moveMusic(to: id)
            if left > 0 {
                self.lastNotice = left == 1
                    ? "1 track only on this device stayed behind"
                    : "\(left) tracks only on this device stayed behind"
            }
        }
    }

    /// Whether `destination` (this device for `nil`) can take the music the
    /// controlled device has: both must play from the same library, and
    /// there must be something to move.
    func canMoveMusic(to destination: DeviceInfo?) -> Bool {
        if destination?.id == mirror.target { return false }
        if destination?.problem != nil { return false }
        if let source = controlled {
            guard source.sameLibrary, source.state != .stopped, source.problem == nil else { return false }
        } else if queue.isEmpty {
            return false
        }
        return destination?.sameLibrary ?? true
    }

    // MARK: - Edit actions
    //
    // Wired to the standard Edit menu, so ⌘A/⌘C/⌘X/⌘V/Delete mean what they
    // mean everywhere else rather than being decorative.

    func removeSelected() {
        remove(itemIds: Array(queueSelection))
        queueSelection = []
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
                lastError = "Nothing there kōan can play."
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
    /// and only rewritten when it changes, because re-serialising a
    /// library-sized queue once a second would be megabytes of writing to
    /// remember one number.
    ///
    /// It runs while the music does. A position that is not advancing is one
    /// already written, so a stopped koan has nothing to save and no reason to
    /// wake once a second for ever to decide that. A queue edited while paused
    /// is written by the edge below rather than by the next tick.
    private var autosave: Task<Void, Never>?

    private func followSession() {
        mirror.follow { [weak self] in
            guard let self else { return }
            let playing = mirror.playhead.playing
            guard playing != (autosave != nil) else { return }
            autosave?.cancel()
            autosave = playing ? Task { [weak self] in await self?.keepSaving() } : nil
            // A pause, a stop or a track change is a moment worth remembering
            // in its own right, and the last one before a long silence.
            if !playing { Task { [weak self] in await self?.save() } }
        }
    }

    private func keepSaving() async {
        while !Task.isCancelled {
            try? await Task.sleep(for: .seconds(1))
            guard !Task.isCancelled else { return }
            await save()
        }
    }

    private func save() async {
        if queueVersion != savedQueueVersion {
            await saveSession()
        } else if currentEntry != nil {
            try? await engine.savePosition()
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
        await settle(within: .milliseconds(500)) { self.mirror.queue.count >= Int(restored) }
    }

    /// Wait for the engine to catch up with something already asked of it, or
    /// give up after `limit`. Woken by the mirror rather than looking again —
    /// see `EngineMirror.waitUntil`.
    func settle(within limit: Duration, _ settled: @escaping @MainActor () -> Bool) async {
        await mirror.waitUntil(.now + limit, settled)
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
    /// changes — but these are ordered against each other on the
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
