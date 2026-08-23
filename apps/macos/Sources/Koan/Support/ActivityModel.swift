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
    struct Task: Identifiable, Equatable {
        let id = UUID()
        /// Shown to the user: "Scanning library", "Syncing with server".
        let label: String
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
        _ work: @escaping @Sendable () throws -> T
    ) async -> Result<T, Error> {
        let id = begin(label)
        defer { end(id) }
        return await _Concurrency.Task.detached(priority: .userInitiated) {
            Result { try work() }
        }.value
    }

    /// Run `work` with a reporter the engine can call back into, so the task
    /// shows a fraction rather than a spinner.
    @discardableResult
    func runReporting<T: Sendable>(
        _ label: String,
        _ work: @escaping @Sendable (EngineProgress) throws -> T
    ) async -> Result<T, Error> {
        let id = begin(label)
        let progress = reporter(for: id)
        defer { end(id) }
        return await _Concurrency.Task.detached(priority: .userInitiated) {
            Result { try work(progress) }
        }.value
    }

    /// For work that does not fit the closure shape — a long-lived poller, or
    /// something whose lifetime is owned elsewhere. Pair every `begin` with an
    /// `end`.
    func begin(_ label: String) -> UUID {
        let task = Task(label: label)
        tasks.append(task)
        return task.id
    }

    func end(_ id: UUID) {
        tasks.removeAll { $0.id == id }
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
    /// so nothing here can wrap them; this polls the flag instead and shows a
    /// row for as long as it is set.
    func mirror(_ label: String, while running: @escaping @Sendable () -> Bool) {
        _Concurrency.Task { [weak self] in
            var id: UUID?
            while !_Concurrency.Task.isCancelled {
                let isRunning = running()
                switch (isRunning, id) {
                case (true, nil):
                    id = self?.begin(label)
                case (false, let some?):
                    self?.end(some)
                    id = nil
                default:
                    break
                }
                try? await _Concurrency.Task.sleep(for: .seconds(1))
            }
        }
    }

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
