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
        /// 0…1 where the engine can say, `nil` where it cannot.
        var progress: Double?
        /// What it is working on right now, if it says.
        var detail: String?
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

    func setProgress(_ id: UUID, _ fraction: Double?) {
        guard let index = tasks.firstIndex(where: { $0.id == id }) else { return }
        tasks[index].progress = fraction
    }

    func setDetail(_ id: UUID, _ detail: String?) {
        guard let index = tasks.firstIndex(where: { $0.id == id }) else { return }
        tasks[index].detail = detail
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
    }

    func advanced(done: UInt64, detail: String) {
        let total = self.total.withLock { $0 }
        let fraction = total > 0 ? Double(done) / Double(total) : nil
        let task = self.task
        Task { @MainActor [weak activity] in
            activity?.setProgress(task, fraction)
            activity?.setDetail(task, detail)
        }
    }
}
