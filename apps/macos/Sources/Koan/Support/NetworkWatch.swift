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
/// flag, a secondary interface coming or going — so only a different status,
/// default interface or gateway counts: a new Wi-Fi network on the same
/// interface is a new gateway. A burst of those, as a handover makes, is told
/// once it settles, and only once there is a network to reconnect on.
final class NetworkWatch: @unchecked Sendable {
    private let engine: KoanEngine
    private let monitor = NWPathMonitor()
    private let queue = DispatchQueue(label: "koan-network-watch")
    /// The last path seen, as status, the default interface and the gateways.
    /// The first update is the network the app started on, which is no change.
    private var current: [String]?
    private var pending: DispatchWorkItem?

    init(engine: KoanEngine) {
        self.engine = engine
        monitor.pathUpdateHandler = { [weak self] path in
            guard let self else { return }
            let next = ["\(path.status)", path.availableInterfaces.first?.name ?? ""]
                + path.gateways.map(\.debugDescription)
            guard let current else {
                self.current = next
                return
            }
            guard next != current else { return }
            self.current = next
            pending?.cancel()
            pending = nil
            guard path.status == .satisfied else { return }
            let tell = DispatchWorkItem { [weak self] in self?.engine.networkChanged() }
            pending = tell
            queue.asyncAfter(deadline: .now() + 1, execute: tell)
        }
        monitor.start(queue: queue)
    }

    deinit {
        monitor.cancel()
    }
}
