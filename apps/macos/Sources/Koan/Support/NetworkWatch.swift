import Foundation
import KoanFFI
import Network

/// Tells the engine when the device's network changes: Wi-Fi to mobile data
/// and back, a cable plugged in, or a network back after none. Connections to the
/// server made on the last network are no use on the next, so the engine makes
/// them afresh and tries a server it had given up on at once rather than when
/// its backoff says. See `koan_core::helpers::network_changed`.
///
/// The system reports a path for every small change — a DNS server, a cost
/// flag, a secondary interface coming or going — so only a different status,
/// default interface or set of gateways counts. Joining another Wi-Fi network
/// drops the interface while it associates, which is a change of its own. A
/// burst of those, as a handover makes, is told once it settles, and only once
/// there is a network to reconnect on.
final class NetworkWatch: @unchecked Sendable {
    /// What of a path counts. The default interface is the first that is not
    /// virtual (`.other`: tunnels, bridges), so a VPN renaming its own leaves
    /// this alone while a move under it does not.
    private struct Route: Equatable {
        let status: NWPath.Status
        let interface: String?
        let gateways: Set<NWEndpoint>

        init(_ path: NWPath) {
            status = path.status
            interface = (path.availableInterfaces.first { $0.type != .other }
                ?? path.availableInterfaces.first)?.name
            gateways = Set(path.gateways)
        }
    }

    private let engine: KoanEngine
    private let monitor = NWPathMonitor()
    private let queue = DispatchQueue(label: "koan-network-watch")
    /// Where the engine is told, one call at a time: renewing the clients and
    /// restarting nearby discovery can take a while, and updates must not wait
    /// behind it.
    private let telling = DispatchQueue(label: "koan-network-changed", qos: .utility)
    /// The last route seen. The first is the network the app started on,
    /// which is no change.
    private var current: Route?
    private var pending: DispatchWorkItem?
    /// When the burst being waited out began: a path that never settles is
    /// told after `longest` all the same.
    private var burst: DispatchTime?
    private static let settle = 1.0
    private static let longest = 5.0

    init(engine: KoanEngine) {
        self.engine = engine
        monitor.pathUpdateHandler = { [weak self] path in
            guard let self else { return }
            let next = Route(path)
            guard let current else {
                self.current = next
                return
            }
            guard next != current else { return }
            self.current = next
            pending?.cancel()
            pending = nil
            // A network gone ends the burst: what comes back settles afresh.
            guard path.status != .unsatisfied else {
                burst = nil
                return
            }
            let tell = DispatchWorkItem { [weak self] in
                guard let self else { return }
                burst = nil
                telling.async { self.engine.networkChanged() }
            }
            pending = tell
            let began = burst ?? .now()
            burst = began
            queue.asyncAfter(deadline: min(.now() + Self.settle, began + Self.longest), execute: tell)
        }
        monitor.start(queue: queue)
    }

    deinit {
        monitor.cancel()
    }
}
