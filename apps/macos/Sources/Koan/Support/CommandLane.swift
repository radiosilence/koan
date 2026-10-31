import Foundation

/// The engine's commands and edits, in the order the person made them.
///
/// The engine runs commands one at a time in the order they reach it, but a
/// call made from here reaches it from Swift's cooperative pool, so two calls
/// made a moment apart can arrive the other way round. A command that first
/// resolves what it acts on (an artist's tracks, a drop) would leave a wider
/// window still. So each command submitted here starts only once the one
/// before it has finished, its resolving included.
///
/// A command submitted from inside one running here, as when a menu resolves
/// an artist and then queues it through `PlayerModel`, runs as part of that
/// one, after whatever it submitted before, rather than behind what the person
/// did in the meantime.
@MainActor
enum CommandLane {
    @MainActor final class Job {
        /// The last command submitted from inside this one.
        var last: Task<Void, Never>?
        var finished = false
    }

    @TaskLocal static var job: Job?

    private static var tail: Task<Void, Never>?

    /// Run `body` after every command submitted before it. The task finishes
    /// when `body` and anything it submitted have.
    @discardableResult
    static func submit(_ body: @escaping @MainActor () async -> Void) -> Task<Void, Never> {
        if let job, !job.finished {
            let previous = job.last
            let task = Task {
                await previous?.value
                await body()
            }
            job.last = task
            return task
        }
        let previous = tail
        let task = Task {
            await previous?.value
            let job = Job()
            await $job.withValue(job) { await body() }
            var awaited: Task<Void, Never>?
            while let last = job.last, last != awaited {
                awaited = last
                await last.value
            }
            job.finished = true
        }
        tail = task
        return task
    }
}
