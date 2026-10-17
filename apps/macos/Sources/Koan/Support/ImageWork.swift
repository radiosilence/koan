import Foundation

/// Where image work happens: off the main actor and off Swift's cooperative
/// pool.
///
/// Decoding a sleeve is tens of milliseconds of CPU and reading one off disk is
/// a blocking syscall. The cooperative pool has as many threads as the machine
/// has cores, and a grid asks for twenty tiles at once, so a screenful of
/// artwork there would take every thread the app has and queue everything
/// unrelated behind it.
///
/// Two lanes, because the two kinds of work want opposite bounds.
enum ImageWork {
    /// Blocking file work. Wide: these threads are asleep in the kernel, not
    /// computing, so having several costs a stack and nothing else. The same
    /// trade koan-ffi's blocking pool makes on the Rust side. At the decode
    /// lane's quality of service, because a sleeve read off disk is one
    /// someone is waiting to see, and on a busy machine utility work can wait
    /// hundreds of milliseconds for a thread.
    private static let disk = DispatchQueue(
        label: "cc.blit.koan.image-disk",
        qos: .userInitiated,
        attributes: .concurrent
    )

    /// Decoding, resampling and hashing. Bounded, because it is CPU-bound:
    /// more of it in flight than the machine can run finishes no sooner and
    /// delays whatever is behind it. Two cores are left for the main thread
    /// and the render server, which is where a scroll is drawn. A fast scroll
    /// through a cold grid wants a hundred sleeves a second or more.
    private static let cpu = Lane(width: max(2, ProcessInfo.processInfo.activeProcessorCount - 2))

    /// How much a piece of work matters to what is on screen. The lane runs the
    /// most urgent first, and the newest first among equals.
    struct Priority: Comparable, Sendable {
        enum Tier: Int, Sendable {
            /// On the way to where a scroll is heading.
            case ahead
            /// Where a flick will come to rest.
            case landing
            /// On screen now.
            case visible
            /// Asked for outside a scroll, such as the window's wash, by
            /// something that is waiting on it.
            case immediate
        }

        let tier: Tier
        /// Points from the screen, or from where a flick will land. Within a
        /// tier the nearest goes first: the row about to scroll in matters
        /// more than one a screen further on.
        var distance: CGFloat = 0

        static let visible = Priority(tier: .visible)
        static let immediate = Priority(tier: .immediate)
        static func landing(_ distance: CGFloat) -> Priority { Priority(tier: .landing, distance: distance) }
        static func ahead(_ distance: CGFloat) -> Priority { Priority(tier: .ahead, distance: distance) }

        static func < (a: Self, b: Self) -> Bool {
            a.tier == b.tier ? a.distance > b.distance : a.tier.rawValue < b.tier.rawValue
        }
    }

    static func onDisk<T: Sendable>(_ body: @escaping @Sendable () -> T) async -> T {
        await withCheckedContinuation { continuation in
            disk.async { continuation.resume(returning: body()) }
        }
    }

    /// Work already started is not abandoned when the caller goes away — the
    /// wash still finishes, and the result is still worth keeping. It goes
    /// ahead of a scroll's sleeves, because whoever asked is waiting on it.
    static func onCPU<T: Sendable>(_ body: @escaping @Sendable () -> T) async -> T {
        await withCheckedContinuation { continuation in
            cpu.submit(priority: { .immediate }, run: { continuation.resume(returning: body()) }, drop: {})
        }
    }

    /// Work that is only worth doing while someone still wants it. Nil when the
    /// caller was cancelled before the work reached the front of the lane: a
    /// tile scrolled past gives its place to the ones on screen. `priority` is
    /// read when the lane picks, so a request can be promoted while it waits.
    static func ifWanted<T: Sendable>(
        priority: @escaping @Sendable () -> Priority,
        _ body: @escaping @Sendable () -> T
    ) async -> T? {
        let ticket = Lane.Ticket()
        return await withTaskCancellationHandler {
            await withCheckedContinuation { (continuation: CheckedContinuation<T?, Never>) in
                cpu.submit(
                    ticket: ticket,
                    priority: priority,
                    run: { continuation.resume(returning: body()) },
                    drop: { continuation.resume(returning: nil) }
                )
            }
        } onCancel: {
            cpu.withdraw(ticket)
        }
    }

    /// A fixed number of workers taking the most wanted job each time one
    /// frees up. A dispatch or operation queue is first come, first served,
    /// which in a fast scroll means decoding the tiles that went past before
    /// the ones that are showing.
    final class Lane: @unchecked Sendable {
        final class Ticket: @unchecked Sendable {
            fileprivate var withdrawn = false
        }

        private struct Job {
            let ticket: Ticket
            let order: UInt64
            let priority: @Sendable () -> Priority
            let run: @Sendable () -> Void
            let drop: @Sendable () -> Void
        }

        private let width: Int
        private let lock = NSLock()
        private var pending: [Job] = []
        private var running = 0
        private var submitted: UInt64 = 0
        private let workers = DispatchQueue(
            label: "cc.blit.koan.image-cpu", qos: .userInitiated, attributes: .concurrent
        )

        init(width: Int) {
            self.width = width
        }

        func submit(
            ticket: Ticket = Ticket(),
            priority: @escaping @Sendable () -> Priority,
            run: @escaping @Sendable () -> Void,
            drop: @escaping @Sendable () -> Void
        ) {
            let dropped: Bool = lock.withLock {
                // Withdrawn before it was even queued: cancelled on the way in.
                if ticket.withdrawn { return true }
                submitted += 1
                pending.append(Job(ticket: ticket, order: submitted, priority: priority, run: run, drop: drop))
                return false
            }
            if dropped { return drop() }
            drain()
        }

        func withdraw(_ ticket: Ticket) {
            let job: Job? = lock.withLock {
                ticket.withdrawn = true
                guard let index = pending.firstIndex(where: { $0.ticket === ticket }) else { return nil }
                return pending.remove(at: index)
            }
            job?.drop()
        }

        private func drain() {
            while let job = next() {
                workers.async { [self] in
                    job.run()
                    lock.withLock { running -= 1 }
                    drain()
                }
            }
        }

        private func next() -> Job? {
            lock.withLock {
                guard running < width, !pending.isEmpty else { return nil }
                var best = 0
                var bestPriority = pending[0].priority()
                for index in pending.indices.dropFirst() {
                    let priority = pending[index].priority()
                    if priority > bestPriority || (priority == bestPriority && pending[index].order > pending[best].order) {
                        best = index
                        bestPriority = priority
                    }
                }
                running += 1
                return pending.remove(at: best)
            }
        }
    }
}
