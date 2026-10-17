import Foundation
import KoanFFI
import Network

/// Tells the engine when the device's network changes: Wi-Fi to mobile data
/// and back, a cable plugged in, or no network at all. Connections to the
/// server made on the last network are no use on the next, so the engine makes
/// them afresh and tries a server it had given up on at once rather than when
/// its backoff says. See `koan_core::helpers::network_changed`.
///
/// The system reports a path for every small change — a DNS server, a cost
/// flag, the same interfaces again — so only a different status or a different
/// set of interfaces counts, and a burst of those, as a Wi-Fi handover makes,
/// is told once it settles.
final class NetworkWatch: @unchecked Sendable {
    private let monitor = NWPathMonitor()
    private let queue = DispatchQueue(label: "koan-network-watch")
    /// The last path seen, as status and interfaces in order of preference.
    /// The first update is the network the app started on, which is no change.
    private var current: [String]?
    private var pending: DispatchWorkItem?

    init(engine: KoanEngine) {
        monitor.pathUpdateHandler = { [weak self] path in
            guard let self else { return }
            let next = ["\(path.status)"] + path.availableInterfaces.map(\.name)
            guard let current else {
                self.current = next
                return
            }
            guard next != current else { return }
            self.current = next
            pending?.cancel()
            let tell = DispatchWorkItem { engine.networkChanged() }
            pending = tell
            queue.asyncAfter(deadline: .now() + 1, execute: tell)
        }
        monitor.start(queue: queue)
    }

    deinit {
        monitor.cancel()
    }
}
