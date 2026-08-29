import Foundation
import KoanFFI
import Observation
import SwiftUI

/// The engine's state, as SwiftUI sees it.
///
/// One object, one loop, no per-model refresh rules. The engine publishes whole
/// slices and this holds the latest of each; views read properties and are
/// invalidated by the framework. Nothing here decides *what* a change means —
/// a slice arrives only when it differs, and the difference is decided in Rust,
/// where every one of these types already has an equality of its own.
///
/// That is the point. Every bug this replaced was a Swift-side copy kept in
/// step with the engine by a rule someone had to remember to write: an album
/// page with an empty cloud after a download landed, a playlist page that heard
/// nothing at all, a progress ring patched into one index and not the other.
/// A snapshot cannot be applied wrongly.
///
/// **`Observable` by hand**, because the macro's sugar is exactly what is in
/// the way: it rewrites stored properties into computed ones that call `access`
/// on read and `withMutation` on write, and this needs the write half driven
/// from a loop rather than from an assignment. `ObservationRegistrar` is
/// public and this is what it is for.
///
/// Two things follow from how observation works, and they decide the shape:
///
/// 1. **Granularity is per keyPath.** Reading `queue` does not subscribe you to
///    `positionMs`. Slices are therefore cut by *rate of change*: the position
///    and the transfer figures move ten times a second and live in properties
///    of their own, so a list that draws neither never hears from them. This is
///    what `PlaybackClock` used to be a whole object for.
/// 2. **It is not per element.** Replacing `queue` invalidates everyone reading
///    it — an array is one property. So anything that moves at its own rate has
///    to leave the array rather than be patched into it, which is why a
///    transfer's byte count is not a field on a queue row.
@MainActor
final class EngineMirror: Observable {
    private let registrar = ObservationRegistrar()

    /// The loop following the engine. Cancelling it ends the subscription.
    private var loop: Task<Void, Never>?

    // MARK: - Slices

    private var _playback = NowPlaying(
        state: .stopped, positionMs: 0, durationMs: 0,
        queueItemId: nil, entry: nil, format: nil, playlistVersion: 0,
        radioEnabled: false
    )
    private var _playhead = Playhead(positionMs: 0, playing: false, at: .now)
    private var _seekableMs: UInt64 = 0
    private var _queue: [QueueItem] = []
    private var _queuedByTrack: [Int64: QueueItem] = [:]
    private var _queuedByPlaylistEntry: [Int64: QueueItem] = [:]
    private var _lock: QueueLock?
    private var _transfers: [Transfer] = []
    private var _figures: [String: TransferFigure] = [:]
    private var _libraryVersion: UInt64 = 0
    private var _scanning = false
    private var _syncing = false

    /// Everything a transport bar shows other than the position. Changes per
    /// track, per state, per format — not per tick.
    var playback: NowPlaying {
        access(\.playback)
        return _playback
    }

    /// Where the playhead was when the engine last said, and whether it has
    /// been moving since.
    ///
    /// An anchor rather than a reading: a playhead advancing at one second per
    /// second is the one thing a client can work out for itself, so the engine
    /// says so when that stops being true — a seek, a pause, a track boundary,
    /// a stall — and not otherwise. Ask it for `at(_:)` rather than reading a
    /// number that would have to keep arriving to stay true.
    var playhead: Playhead {
        access(\.playhead)
        return _playhead
    }

    /// How far into the current track a seek can land. Equal to the duration
    /// for anything on disk, and short of it while a download is still
    /// arriving — the engine clamps a seek to the same extent, so a scrub past
    /// this would land somewhere the thumb was never dragged to.
    var seekableMs: UInt64 {
        access(\.seekableMs)
        return _seekableMs
    }

    /// What koan is doing on its own. One property for both, because the only
    /// thing that reads them draws a row for each and would be invalidated by
    /// either.
    var tasks: EngineTasks {
        access(\.tasks)
        return EngineTasks(scanning: _scanning, syncing: _syncing)
    }

    var queue: [QueueItem] {
        access(\.queue)
        return _queue
    }

    /// Queue entries indexed by library track id, so a library row can say what
    /// the queue knows about it without scanning the queue once per row.
    ///
    /// Observed as `queue`, not as itself: it is derived from that array and
    /// moves with it, so giving it an identity of its own would only invite the
    /// two to be invalidated separately. Two indexes over one array that must
    /// be patched in step is what broke playlist progress; derived from a single
    /// slice they cannot disagree.
    var queuedByTrack: [Int64: QueueItem] {
        access(\.queue)
        return _queuedByTrack
    }

    /// Queue entries indexed by the playlist row they came from.
    ///
    /// Keyed on the entry rather than the track, because a playlist may hold the
    /// same track twice and `queuedByTrack` can only answer for one of them.
    /// This is what lets a playlist row ask about *itself*.
    var queuedByPlaylistEntry: [Int64: QueueItem] {
        access(\.queue)
        return _queuedByPlaylistEntry
    }

    /// What the queue still is, while it is still a playlist or a record.
    var lock: QueueLock? {
        access(\.lock)
        return _lock
    }

    /// Every transfer koan knows about — running first, then whatever settled
    /// most recently. Structural: see `figure(for:)` for the numbers.
    var transfers: [Transfer] {
        access(\.transfers)
        return _transfers
    }

    /// Bumped whenever the library's rows change — a scan, a sync, an import,
    /// an organize, a playlist edit, a download landing.
    ///
    /// A signal, not a mirror. A record's tracks and a search's results are
    /// asked for on demand, so what they want is to know to ask again — which
    /// in SwiftUI is `.task(id: mirror.libraryVersion)` on the page showing
    /// them. The page that draws a thing is the thing that reloads it, so
    /// there is no model to forget one.
    var libraryVersion: UInt64 {
        access(\.libraryVersion)
        return _libraryVersion
    }

    // MARK: - Reading the fast slice
    //
    // Behind calls rather than a property, so a view has to mean it. Reading
    // any of these subscribes the caller to a figure that moves ten times a
    // second while anything is downloading — right for a row drawing a ring,
    // wrong for the rest of a list.

    /// The byte counts, keyed by queue item.
    private var figures: [String: TransferFigure] {
        access(\.figures)
        return _figures
    }

    /// How far one transfer has got, if it is one koan is running.
    func figure(for queueItemId: String) -> TransferFigure? {
        figures[queueItemId]
    }

    /// 0–1 through a transfer, or `nil` when the server never said how big it
    /// was and there is no fraction to draw.
    func progress(for queueItemId: String) -> Double? {
        figure(for: queueItemId)?.progress
    }

    /// How many transfers are actually moving. What the sidebar counts.
    var activeTransfers: Int {
        access(\.transfers)
        return _transfers.count(where: { !$0.state.isSettled })
    }

    /// Whether anything has settled and could be cleared away.
    var hasSettledTransfers: Bool {
        access(\.transfers)
        return _transfers.contains { $0.state.isSettled }
    }

    // MARK: - Following

    /// Follow the engine for as long as this mirror is alive.
    ///
    /// The loop *is* the subscription — there is nothing to register and
    /// nothing to unregister. A batch is everything that changed inside one of
    /// the engine's ticks, so a frame's worth of movement lands as one pass and
    /// SwiftUI sees a single consistent state rather than six.
    func start(engine: KoanEngine) {
        loop?.cancel()
        loop = Task { [weak self] in
            let stream = engine.observe()
            while let batch = await stream.next() {
                guard let self else { return }
                for slice in batch { self.apply(slice) }
            }
        }
    }

    /// Take a slice. Every one that arrives differs from the one held — the
    /// engine already decided that — so this is an assignment and a mutation
    /// notice, and never a merge.
    private func apply(_ slice: StateSlice) {
        switch slice {
        case .playback(let nowPlaying):
            mutate(\.playback) { _playback = nowPlaying }
        case .playhead(let ms, let seekableMs, let playing):
            // Stamped on arrival. The engine cannot send a clock across the
            // boundary and does not need to: the message is minutes old only
            // if it sat in a queue, and it did not.
            mutate(\.playhead) { _playhead = Playhead(positionMs: ms, playing: playing, at: .now) }
            if seekableMs != _seekableMs {
                mutate(\.seekableMs) { _seekableMs = seekableMs }
            }
        case .queue(let items):
            mutate(\.queue) {
                _queue = items
                _queuedByTrack = Dictionary(
                    items.compactMap { item in item.trackId.map { ($0, item) } },
                    // A track queued twice: prefer the entry that is actually
                    // doing something over one still sitting idle.
                    uniquingKeysWith: { a, b in b.status == .queued ? a : b }
                )
                _queuedByPlaylistEntry = Dictionary(
                    items.compactMap { item in item.playlistEntryId.map { ($0, item) } },
                    uniquingKeysWith: { a, b in b.status == .queued ? a : b }
                )
            }
        case .lock(let lock):
            mutate(\.lock) { _lock = lock }
        case .transfers(let transfers):
            mutate(\.transfers) { _transfers = transfers }
        case .figures(let figures):
            mutate(\.figures) {
                _figures = Dictionary(
                    figures.map { ($0.queueItemId, $0) },
                    uniquingKeysWith: { first, _ in first }
                )
            }
        case .library(let version):
            mutate(\.libraryVersion) { _libraryVersion = version }
        case .tasks(let scanning, let syncing):
            if scanning != _scanning || syncing != _syncing {
                mutate(\.tasks) {
                    _scanning = scanning
                    _syncing = syncing
                }
            }
        }
    }

    // MARK: - Observation plumbing
    //
    // What the `@Observable` macro would have written, minus the sugar that
    // insists a mutation is an assignment. A keyPath is an identity here and
    // nothing more — it is never read through.

    private func access<V>(_ keyPath: KeyPath<EngineMirror, V>) {
        registrar.access(self, keyPath: keyPath)
    }

    private func mutate<V>(_ keyPath: KeyPath<EngineMirror, V>, _ body: () -> Void) {
        registrar.withMutation(of: self, keyPath: keyPath) { body() }
    }
}

extension EngineMirror {
    /// Run `body` now, and again whenever anything it read on this mirror
    /// changes.
    ///
    /// For the handful of things that are not views and so have no body to be
    /// invalidated — Control Center, chiefly. `withObservationTracking` fires
    /// its callback *before* the mutation completes and unregisters itself, so
    /// the hop is both what lets the new value be read and what re-arms it.
    func follow(_ body: @escaping @MainActor () -> Void) {
        withObservationTracking(body) {
            Task { @MainActor [weak self] in self?.follow(body) }
        }
    }

    /// Wait until `settled` holds, or until `deadline` passes.
    ///
    /// For the two places that have asked the engine for something and want the
    /// answer before drawing — a restored queue, a queue mutation to confirm.
    /// Both looked again every five milliseconds until it arrived; the mirror
    /// is observable, so the arrival is a thing to be woken by. The deadline is
    /// one sleep for the whole wait rather than one per look, and it is a
    /// giving-up clock, not a checking one.
    func waitUntil(
        _ deadline: ContinuousClock.Instant,
        _ settled: @escaping @MainActor () -> Bool
    ) async {
        while !settled(), ContinuousClock.now < deadline {
            await nextChange(before: deadline)
        }
    }

    /// Resumes on the next change to anything held here, or at `deadline`.
    ///
    /// Resumed from a task rather than from the change handler itself: the
    /// handler runs *before* the mutation it is announcing, so a caller told
    /// there and then would read the value it was waiting to see change.
    ///
    /// Whichever arrives first resumes; the `Once` is what makes the other one
    /// harmless, since observation is registered for one change and a handler
    /// that lost the race still fires.
    private func nextChange(before deadline: ContinuousClock.Instant) async {
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            let once = Once()
            withObservationTracking {
                _ = playback
                _ = playhead
                _ = queue
                _ = lock
                _ = transfers
                _ = figures
                _ = libraryVersion
                _ = tasks
            } onChange: {
                Task { @MainActor in
                    if once.take() { continuation.resume() }
                }
            }
            Task { @MainActor in
                try? await Task.sleep(until: deadline, clock: .continuous)
                if once.take() { continuation.resume() }
            }
        }
    }
}

/// One-shot latch. Everything that touches it is on the main actor, so it is
/// a Bool and a method rather than anything cleverer.
@MainActor
private final class Once {
    private var spent = false

    func take() -> Bool {
        guard !spent else { return false }
        spent = true
        return true
    }
}

extension TransferState {
    var isSettled: Bool { self == .done || self == .failed }
}

extension View {
    /// Run `action` when this view appears, when `id` changes, and again
    /// whenever the library's rows change.
    ///
    /// For everything asked for on demand rather than mirrored: a record's
    /// tracks, a playlist's rows, the library's counts. The page that draws a
    /// thing is the thing that reloads it, so there is no model holding a rule
    /// about which pages to refresh — and no page that rule can miss. Missing
    /// one is what left an album page with an empty cloud after a download
    /// landed, and a playlist page with nothing at all.
    func reloading<ID: Equatable>(
        on id: ID,
        _ action: @escaping () async -> Void
    ) -> some View {
        modifier(ReloadOnLibraryChange(id: id, action: action))
    }
}

private struct ReloadOnLibraryChange<ID: Equatable>: ViewModifier {
    let id: ID
    let action: () async -> Void

    @Environment(EngineMirror.self) private var mirror

    func body(content: Content) -> some View {
        content.task(id: Key(id: id, version: mirror.libraryVersion)) { await action() }
    }

    private struct Key: Equatable {
        let id: ID
        let version: UInt64
    }
}

/// What koan is doing on its own, as one value: the only thing that reads
/// either draws a row for each, and would be invalidated by both.
struct EngineTasks: Equatable {
    /// The startup scan or the watched folders finding something.
    let scanning: Bool
    /// The automatic sync with a server.
    let syncing: Bool
}

/// Where the playhead was, and whether it is still moving.
///
/// The engine publishes one of these when a client's own reckoning would go
/// wrong, so everything that shows a position derives it from here rather than
/// waiting to be told a number. Nothing in koan ticks to keep a position up to
/// date; the anchor is what makes that possible.
struct Playhead: Equatable {
    let positionMs: UInt64
    let playing: Bool
    /// When this arrived. Local, because it is compared only with `Date.now`.
    let at: Date

    /// Where the playhead is at `date`, by this anchor's reckoning.
    func at(_ date: Date = .now) -> UInt64 {
        guard playing else { return positionMs }
        return positionMs + UInt64(max(0, date.timeIntervalSince(at)) * 1000)
    }

    /// The same, capped at a track's length — a prediction runs past the end
    /// of a track for as long as it takes the engine to say the next one has
    /// started.
    func at(_ date: Date = .now, within durationMs: UInt64) -> UInt64 {
        durationMs > 0 ? min(at(date), durationMs) : at(date)
    }
}
