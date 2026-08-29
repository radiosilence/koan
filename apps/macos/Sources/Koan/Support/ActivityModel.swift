import Foundation
import KoanFFI
import Observation
import os

/// Everything slow that is happening right now.
///
/// A scan, a remote sync, a large queue add and a library rebuild all take
/// anywhere from a second to a minute, and each had its own flag — or none at
/// all — so pressing the button looked like nothing happened. One registry, one
/// place to show it, and adding a task later means calling `run` rather than
/// inventing another boolean.
@MainActor
@Observable
final class ActivityModel {
    /// What a task has hold of, so the next one can be told apart from the
    /// ones it would actually collide with.
    ///
    /// This was a single flag: any library task running greyed out every other,
    /// on the grounds that they all queue behind SQLite's one writer. Most of
    /// them never meet, though — a sync writes the server's rows while a scan
    /// writes your files' — and sitting out a twenty-minute sync before you can
    /// rescan a folder buys nothing. So a task says what it touches and only
    /// what would touch the same is disabled.
    struct Resources: OptionSet, Sendable {
        let rawValue: Int

        /// The files on disk: read for their tags, or moved.
        static let localFiles = Resources(rawValue: 1 << 0)
        /// Library rows for files on this machine.
        static let localTracks = Resources(rawValue: 1 << 1)
        /// Library rows mirrored from the server.
        static let remoteTracks = Resources(rawValue: 1 << 2)
        /// The downloaded copies of remote tracks.
        static let downloads = Resources(rawValue: 1 << 3)

        /// Reading files and writing their rows — a scan, a drop, a move.
        static let localLibrary: Resources = [.localFiles, .localTracks]
        /// For the tasks that empty the library or rewrite all of it.
        static let wholeLibrary: Resources = [.localFiles, .localTracks, .remoteTracks, .downloads]
    }

    struct Task: Identifiable, Equatable {
        let id = UUID()
        /// Shown to the user: "Scanning library", "Syncing with server".
        let label: String
        /// What starting another task would have to wait for. Empty for work
        /// small enough that nothing need wait on it — a queue edit, a sign-in.
        var uses: Resources = []
        /// Whether asking it to stop does anything.
        var cancellable = false
        /// How many done and how many there are, where the engine counts them.
        /// Shown as "12,345 / 48,087" — a percentage alone hides whether the
        /// remaining work is ten items or ten thousand.
        var done: UInt64?
        var total: UInt64?
        /// What it is working on right now, if it says.
        var detail: String?

        /// 0…1, or nil when there is nothing to divide by.
        var progress: Double? {
            guard let done, let total, total > 0 else { return nil }
            return min(1, Double(done) / Double(total))
        }
    }

    private(set) var tasks: [Task] = []

    var isBusy: Bool { !tasks.isEmpty }

    /// Everything the running tasks between them have hold of.
    ///
    /// Stored rather than derived from `tasks`. `.commands` is part of the Scene
    /// body, so a menu that reads this would rebuild on every progress tick if
    /// it depended on the task list — observation is per property, and this one
    /// only changes when a task starts or ends.
    private(set) var busy: Resources = []

    /// Whether starting something that needs `wanted` would collide with what
    /// is already running. What every menu item and button gates on.
    func conflicts(with wanted: Resources) -> Bool {
        !busy.isDisjoint(with: wanted)
    }

    private func refreshBusy() {
        let held = tasks.reduce(into: Resources()) { $0.formUnion($1.uses) }
        if held != busy { busy = held }
    }

    /// Called to stop the running scan. Set by whoever owns the engine.
    ///
    /// The scanner is the only thing that reads the engine's cancel flag, so
    /// only a scan is ever marked `cancellable` — a cancel button on any other
    /// row would stop a scan running beside it instead of the task it sits on.
    var cancelLibraryTask: (() -> Void)?

    func cancel(_ id: UUID) {
        guard let task = tasks.first(where: { $0.id == id }), task.cancellable else { return }
        cancelLibraryTask?()
    }

    /// The one to show when there is only room for one. The oldest, so a long
    /// sync is not hidden by a queue add that started after it.
    var current: Task? { tasks.first }

    /// Run `work` off the main actor with a task registered for its duration.
    ///
    /// The registration is removed however `work` ends, including by throwing,
    /// so a failure cannot leave a spinner running forever.
    @discardableResult
    func run<T: Sendable>(
        _ label: String,
        uses: Resources = [],
        cancellable: Bool = false,
        _ work: @escaping @Sendable () async throws -> T
    ) async -> Result<T, Error> {
        let id = begin(label, uses: uses, cancellable: cancellable)
        defer { end(id) }
        do {
            return .success(try await work())
        } catch {
            return .failure(error)
        }
    }

    /// Run `work` with a reporter the engine can call back into, so the task
    /// shows a fraction rather than a spinner.
    @discardableResult
    func runReporting<T: Sendable>(
        _ label: String,
        uses: Resources = [],
        cancellable: Bool = true,
        _ work: @escaping @Sendable (EngineProgress) async throws -> T
    ) async -> Result<T, Error> {
        let id = begin(label, uses: uses, cancellable: cancellable)
        let progress = reporter(for: id)
        defer { end(id) }
        do {
            return .success(try await work(progress))
        } catch {
            return .failure(error)
        }
    }

    /// For work that does not fit the closure shape — a long-lived poller, or
    /// something whose lifetime is owned elsewhere. Pair every `begin` with an
    /// `end`.
    func begin(_ label: String, uses: Resources = [], cancellable: Bool = false) -> UUID {
        let task = Task(label: label, uses: uses, cancellable: cancellable)
        tasks.append(task)
        refreshBusy()
        return task.id
    }

    func end(_ id: UUID) {
        tasks.removeAll { $0.id == id }
        refreshBusy()
    }

    func setCounts(_ id: UUID, done: UInt64?, total: UInt64?) {
        guard let index = tasks.firstIndex(where: { $0.id == id }) else { return }
        if let done { tasks[index].done = done }
        if let total { tasks[index].total = total }
    }

    func setDetail(_ id: UUID, _ detail: String?) {
        guard let index = tasks.firstIndex(where: { $0.id == id }) else { return }
        tasks[index].detail = detail
    }

    /// Mirror a background job the engine runs on its own — the startup scan,
    /// the folder watcher, the automatic sync. They are not started by the UI,
    /// so nothing here can wrap them: the engine says whether each is running
    /// and this shows a row for as long as it is.
    ///
    /// Called with each answer as it arrives rather than asking for it. It
    /// asked once a second, for ever, so that a row would appear within a
    /// second of a scan starting — which is a wake a second for the whole life
    /// of the app to notice something that happens twice a day.
    func mirror(
        _ label: String,
        uses: Resources = [],
        cancellable: Bool = false,
        running: Bool
    ) {
        let existing = mirrored[label]
        switch (running, existing) {
        case (true, nil):
            mirrored[label] = begin(label, uses: uses, cancellable: cancellable)
        case (false, let some?):
            end(some)
            mirrored[label] = nil
        default:
            break
        }
    }

    /// The row standing for each mirrored job, by its label.
    private var mirrored: [String: UUID] = [:]

    /// A `ProgressReporter` the engine can call from its worker threads,
    /// forwarding onto the main actor.
    func reporter(for id: UUID) -> EngineProgress {
        EngineProgress(activity: self, task: id)
    }
}

/// Bridges the engine's progress callbacks onto the main actor.
///
/// The engine calls these from whichever thread is doing the work, so every
/// hop is explicit. koan already throttles them — roughly one call per sixty-odd
/// files — so this does not need to throttle again.
final class EngineProgress: ProgressReporter, @unchecked Sendable {
    private weak var activity: ActivityModel?
    private let task: UUID
    private let total = OSAllocatedUnfairLock(initialState: UInt64(0))

    init(activity: ActivityModel, task: UUID) {
        self.activity = activity
        self.task = task
    }

    func started(total: UInt64) {
        self.total.withLock { $0 = total }
        let task = self.task
        Task { @MainActor [weak activity] in
            activity?.setCounts(task, done: 0, total: total)
        }
    }

    func advanced(done: UInt64, detail: String) {
        let total = self.total.withLock { $0 }
        let task = self.task
        Task { @MainActor [weak activity] in
            activity?.setCounts(task, done: done, total: total > 0 ? total : nil)
            activity?.setDetail(task, detail)
        }
    }
}
