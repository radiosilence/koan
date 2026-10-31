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
    /// Where invites go, Tailscale, and the addresses Settings shows, as the
    /// engine last worked them out. `nil` until first asked.
    private(set) var status: ShareStatus?
    /// Tailscale on this Mac, while it runs.
    var tailscale: TailscaleStatus? { status?.tailscale }
    /// Tailscale has been looked for, so `nil` means it is not here.
    var tailscaleChecked: Bool { status != nil }
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
            await start(port: settings.sharePort)
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

    func start(port: UInt16) async {
        busy = true
        defer { busy = false }
        do {
            sharing = try await engine.startServer(port: port, names: Self.names)
            failure = nil
        } catch {
            sharing = nil
            failure = Self.reason(error)
        }
        // A start publishes the Funnel again where it was left on, and may
        // move its address.
        revision += 1
        await refreshStatus()
    }

    func refreshStatus() async {
        status = await engine.shareStatus()
    }

    /// Where invites send phones. `.funnel` publishes through Tailscale
    /// Funnel; `.custom` takes `address`, or keeps the one set.
    func setInviteTo(_ to: InviteTo, address: String? = nil) async {
        busy = true
        defer { busy = false }
        do {
            try await engine.setInviteTo(to: to, address: address)
            funnelError = nil
        } catch {
            funnelError = Self.reason(error)
        }
        revision += 1
        await refreshStatus()
    }

    /// `nil` when changed, else why not.
    func resetPassword(_ password: String) async -> String? {
        do {
            try await engine.resetServerAdminPassword(password: password)
            return nil
        } catch {
            return Self.reason(error)
        }
    }

    /// Read the addresses again, after a change of network.
    func refresh() async {
        guard sharing != nil else { return }
        sharing = await engine.serverSharing()
        await refreshStatus()
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
