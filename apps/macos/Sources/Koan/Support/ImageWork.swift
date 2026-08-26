import Foundation

/// Where image work happens, which is neither of the two places it was.
///
/// Decoding a sleeve is tens of milliseconds of CPU and reading one off disk is
/// a blocking syscall. Both used to run on Swift's *cooperative* pool, which has
/// as many threads as the machine has cores — eight here — and a grid asks for
/// twenty tiles at once. So a screenful of artwork took every thread the app
/// had, and everything unrelated queued behind it: a 128µs database read came
/// back in a hundred milliseconds, and the statement after it waited two seconds
/// to run at all. Extracting a record's colour was worse still, on the main
/// actor.
///
/// Two lanes, because the two kinds of work want opposite bounds.
enum ImageWork {
    /// Blocking file work. Wide: these threads are asleep in the kernel, not
    /// computing, so having several costs a stack and nothing else. The same
    /// trade koan-ffi's blocking pool makes on the Rust side.
    private static let disk = DispatchQueue(
        label: "cc.blit.koan.image-disk",
        qos: .utility,
        attributes: .concurrent
    )

    /// Decoding, resampling and hashing. Bounded, because it is CPU-bound:
    /// more of it in flight than the machine can run finishes no sooner and
    /// delays whatever is behind it. Half the cores leaves room for the app to
    /// keep drawing while a grid fills.
    private static let cpu: OperationQueue = {
        let queue = OperationQueue()
        queue.maxConcurrentOperationCount = max(2, ProcessInfo.processInfo.activeProcessorCount / 2)
        queue.qualityOfService = .userInitiated
        return queue
    }()

    static func onDisk<T: Sendable>(_ body: @escaping @Sendable () -> T) async -> T {
        await withCheckedContinuation { continuation in
            disk.async { continuation.resume(returning: body()) }
        }
    }

    /// Work already started is not abandoned when the caller goes away — a tile
    /// scrolled past still finishes, and the result is still worth caching for
    /// when it scrolls back.
    static func onCPU<T: Sendable>(_ body: @escaping @Sendable () -> T) async -> T {
        await withCheckedContinuation { continuation in
            cpu.addOperation { continuation.resume(returning: body()) }
        }
    }
}

