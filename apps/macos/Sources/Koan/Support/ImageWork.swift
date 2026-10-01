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


