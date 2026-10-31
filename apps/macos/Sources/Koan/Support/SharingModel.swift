#if os(macOS)
import KoanFFI
import SwiftUI
import SystemConfiguration

/// This Mac serving its own library to the person's other devices, as a kōan
/// server: Settings → Server → Share this Mac.
///
/// Whether sharing is on is a setting (`share.enabled`), written through
/// `SettingsModel` like every other; this holds what is running.
@MainActor
@Observable
final class SharingModel {
    private let engine: KoanEngine
    /// Where the library is served, while it is.
    private(set) var sharing: Sharing?
    /// Why the last start failed, until the next start or stop.
    private(set) var failure: String?
    /// The account the library is shared under, its first admin. `nil` until
    /// one is made; sharing cannot start before.
    private(set) var account: String?
    /// A start or stop is under way.
    private(set) var busy = false
    /// Tailscale on this Mac, while it runs; `nil` until asked or when not.
    private(set) var tailscale: TailscaleStatus?
    /// Tailscale has been looked for, so `nil` means it is not here.
    private(set) var tailscaleChecked = false
    /// Why the last Funnel change failed.
    private(set) var funnelError: String?
    /// Moves whenever the engine itself wrote a setting (the public address
    /// Funnel publishes at), so Settings reads them again rather than writing
    /// its older copy back.
    private(set) var revision = 0

    init(engine: KoanEngine) {
        self.engine = engine
    }

    /// Read the account, and serve again if sharing was left on.
    func resume() async {
        account = try? await engine.serverAdmin()
        let settings = await engine.settings()
        if settings.shareEnabled, account != nil {
            await start(port: settings.sharePort, publicAddress: settings.serverUrl)
        }
    }

    /// Make the account the library is shared under. `nil` when made, else
    /// why not.
    func createAccount(username: String, password: String) async -> String? {
        do {
            try await engine.createFirstServerAccount(username: username, password: password)
            account = try await engine.serverAdmin()
            return nil
        } catch {
            return Self.reason(error)
        }
    }

    /// `publicAddress` is the tunnel's address, if any: a name the server
    /// answers to as well as this Mac's own.
    func start(port: UInt16, publicAddress: String) async {
        busy = true
        defer { busy = false }
        var names = Self.names
        if let host = URL(string: publicAddress)?.host() { names.append(host) }
        do {
            sharing = try await engine.startServer(port: port, names: names)
            failure = nil
        } catch {
            sharing = nil
            failure = Self.reason(error)
        }
        // A start publishes the Funnel again where it was left on, and may
        // move its address.
        revision += 1
        await refreshTailscale()
    }

    func refreshTailscale() async {
        tailscale = await engine.tailscale()
        tailscaleChecked = true
    }

    /// Publish on the internet through Tailscale Funnel, and send invites there.
    func enableFunnel() async {
        busy = true
        defer { busy = false }
        do {
            _ = try await engine.enableFunnel()
            funnelError = nil
        } catch {
            funnelError = Self.reason(error)
        }
        revision += 1
        await refreshTailscale()
    }

    func disableFunnel() async {
        busy = true
        defer { busy = false }
        do {
            try await engine.disableFunnel()
            funnelError = nil
        } catch {
            funnelError = Self.reason(error)
        }
        revision += 1
        await refreshTailscale()
    }

    /// Read the network address again, after a change of network.
    func refresh() async {
        guard sharing != nil else { return }
        sharing = await engine.serverSharing()
    }

    func stop() async {
        busy = true
        defer { busy = false }
        // Gone at once from everything that shows it; the server drains behind.
        sharing = nil
        failure = nil
        await engine.stopServer()
    }

    /// What the server answers to besides IP addresses: this Mac's Bonjour
    /// name, which a phone on the network can reach it by.
    private static var names: [String] {
        guard let name = SCDynamicStoreCopyLocalHostName(nil) as String? else { return [] }
        return ["\(name).local"]
    }

    private static func reason(_ error: Error) -> String {
        switch error {
        case let KoanError.Refused(message), let KoanError.BadArgument(message),
             let KoanError.Database(message):
            message
        default:
            error.localizedDescription
        }
    }
}
#endif
