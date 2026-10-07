import Foundation
import KoanFFI
import Observation

/// The settings window's copy of the configuration.
///
/// Read once when the window opens and written when a field is committed, not
/// on every keystroke — `config.toml` is shared with the CLI and the TUI, and
/// rewriting it while someone types a URL means a half-typed value is briefly
/// the live configuration for anything reading it.
///
/// Reloaded when the window regains focus, so a change made in the TUI shows up
/// rather than being silently overwritten by whatever this window last saw.
@MainActor
@Observable
final class SettingsModel {
    private let engine: KoanEngine
    private let activity: ActivityModel
    private let art: CoverArtCache?

    private(set) var settings: Settings
    private(set) var lastError: String?
    /// What the last thing done here came to. Gone after a few seconds, as
    /// the app's other notices are; an error stays until the next action.
    private(set) var lastResult: String? {
        didSet {
            dismissal?.cancel()
            guard let shown = lastResult else { return }
            dismissal = Task { [weak self] in
                guard (try? await Task.sleep(for: .seconds(6))) != nil else { return }
                if self?.lastResult == shown { self?.lastResult = nil }
            }
        }
    }
    @ObservationIgnored private var dismissal: Task<Void, Never>?

    /// Typed here rather than in `settings`, because it never comes back out of
    /// the engine — the credential store is write-only from this side.
    var password = ""
    /// Whether `password` holds an API key made in the web UI rather than the
    /// account's password: one that can be revoked on its own, and the only
    /// thing worth typing on a device with no keyboard.
    var withApiKey = false

    init(engine: KoanEngine, activity: ActivityModel, art: CoverArtCache?) async {
        self.engine = engine
        self.activity = activity
        self.art = art
        self.settings = await engine.settings()
    }

    func reload() {
        Task { settings = await engine.settings() }
    }

    /// Let `account` on this server control this device, or stop. The list
    /// arrives through the engine's connection slice once the server has it.
    func shareDevice(with account: String, allow: Bool) throws {
        try engine.shareDevice(grantee: account, allow: allow)
    }

    /// Hang up a device connected to this one on the network. Refusing it as
    /// well keeps its address from connecting again.
    func endConnection(_ connection: ConnectedInfo, refuse: Bool) {
        if refuse, let addr = connection.addr, !settings.devicesRefused.contains(addr) {
            edit { $0.devicesRefused.append(addr) }
        }
        if let key = connection.key {
            engine.endConnection(key: key)
        }
    }

    /// Let a disconnected device back in.
    func allowHeld(_ held: HeldInfo) {
        engine.allowHeld(addr: held.addr)
    }

    // MARK: - Play queue on the server

    /// What the server holds, asked before keeping the queue there: that
    /// replaces this device's queue with it.
    func serverQueue() async -> ServerQueue? {
        do {
            return try await engine.serverQueue()
        } catch {
            lastError = Self.describe(error)
            return nil
        }
    }

    func setServerQueue(_ on: Bool) async {
        do {
            try await engine.setServerQueue(on: on)
            lastError = nil
        } catch {
            lastError = Self.describe(error)
        }
        settings = await engine.settings()
    }

    // MARK: - Editing

    /// Mutate a field and write the result. Every control commits through here,
    /// so there is one place that decides when the file is touched.
    func edit(_ change: (inout Settings) -> Void) {
        var next = settings
        change(&next)
        settings = next
        commit()
    }

    private func commit() {
        Task {
            do {
                try await engine.updateSettings(s: settings)
                lastError = nil
            } catch {
                lastError = Self.describe(error)
            }
        }
    }

    // MARK: - Folders

    /// Add folders to scan, ignoring any already listed.
    func addFolders(_ paths: [String]) {
        edit { s in
            for path in paths where !s.libraryFolders.contains(where: { $0.path == path }) {
                // Count comes back from the engine on the next read; the scan
                // this kicks off is what fills it in.
                s.libraryFolders.append(LibraryFolder(path: path, tracks: 0))
            }
        }
        scan()
    }

    /// Stop scanning a folder, and optionally forget what it put in the library.
    ///
    /// Keeping the rows leaves records on screen whose files koan will never
    /// look at again; forgetting them is what makes "remove every folder and
    /// sign out" end at an empty library, which is what people expect of it.
    func removeFolder(_ path: String, forgetTracks: Bool) {
        edit { $0.libraryFolders.removeAll { $0.path == path } }
        guard forgetTracks else { return }
        let engine = self.engine
        Task {
            let result = await activity.run(
                "Forgetting \(URL(fileURLWithPath: path).lastPathComponent)",
                uses: [.localTracks]
            ) {
                try await engine.forgetFolder(path: path)
            }
            switch result {
            case .success(let n): lastResult = "Forgot \(n.formatted(.number)) tracks"
            case .failure(let e): lastError = Self.describe(e)
            }
            reload()
        }
    }

    // MARK: - Actions

    func scan(force: Bool = false) {
        let engine = self.engine
        Task {
            let result = await activity.runReporting(
                force ? "Rescanning every file" : "Scanning library",
                uses: .localLibrary
            ) { progress in
                try await engine.scanReporting(force: force, reporter: progress)
            }
            switch result {
            case .success(let s):
                lastResult = "\(s.added) added · \(s.updated) updated · \(s.removed) removed"
            case .failure(let e):
                lastError = Self.describe(e)
            }
            reload()
        }
    }

    func signIn(url: String, username: String) {
        let engine = self.engine
        let password = self.password
        let withKey = self.withApiKey
        Task {
            let result = await activity.run("Signing in") {
                if withKey {
                    try await engine.signInRemoteWithKey(url: url, username: username, apiKey: password)
                } else {
                    try await engine.signInRemote(url: url, username: username, password: password)
                }
            }
            switch result {
            case .success:
                self.password = ""
                lastError = nil
                lastResult = "Signed in to \(url)"
                #if os(iOS)
                // A server to send notifications now exists; ask to show them.
                PushDelegate.requestAlertsIfSignedIn()
                #endif
            case .failure(let e):
                lastError = Self.describe(e)
            }
            reload()
        }
    }

    func signOut(forgetTracks: Bool) {
        Task {
            do {
                try await engine.signOutRemote()
                lastResult = "Signed out"
                lastError = nil
                NotificationCenter.default.post(name: .koanSignedOut, object: nil)
            } catch {
                lastError = Self.describe(error)
                reload()
                return
            }
            guard forgetTracks else {
                reload()
                return
            }
            // Local rows too: it takes the server off the ones that were on
            // both, so a scan writing them would be writing the same rows.
            let result = await activity.run(
                "Forgetting the server's tracks",
                uses: [.remoteTracks, .localTracks]
            ) {
                try await self.engine.forgetRemote()
            }
            switch result {
            case .success(let n): lastResult = "Signed out and forgot \(n.formatted(.number)) tracks"
            case .failure(let e): lastError = Self.describe(e)
            }
            reload()
        }
    }

    func syncNow() {
        let engine = self.engine
        Task {
            let result = await activity.run(
                "Syncing with server",
                uses: [.remoteTracks],
                followsSync: true
            ) {
                try await engine.syncRemote()
            }
            switch result {
            case .success(let s):
                // "0 tracks across 0 albums" reads as a failure, and is what
                // an empty server answers.
                lastResult = s.tracks == 0 && s.favouritesImported == 0
                    ? "Already up to date"
                    : "\(s.tracks.formatted(.number)) tracks across \(s.albums.formatted(.number)) albums"
            case .failure(let e):
                lastError = Self.describe(e)
            }
            reload()
        }
    }

    func clearCache() {
        let engine = self.engine
        Task {
            let result = await activity.run("Clearing downloads", uses: [.downloads]) {
                try await engine.clearDownloadCache()
            }
            switch result {
            case .success(let c):
                lastResult = "Freed \(Format.bytes(Int64(c.bytes))) across \(c.files) files"
            case .failure(let e):
                lastError = Self.describe(e)
            }
            reload()
        }
    }

    func rebuildIndex() {
        let engine = self.engine
        Task {
            let result = await activity.run("Clearing the library index", uses: .wholeLibrary) {
                try await engine.rebuildIndex()
            }
            switch result {
            case .success(let s):
                // Artwork is cached by album, track and artist id, and the
                // rebuilt library hands those ids out again from 1.
                art?.purge()
                lastResult = "Removed \(s.tracks) tracks — scan or sync to rebuild"
            case .failure(let e):
                lastError = Self.describe(e)
            }
            reload()
        }
    }

    /// Change the signed-in account's password. This device stays signed in;
    /// the account's others have to sign in again.
    func changePassword(current: String, new: String) async -> Bool {
        do {
            try await engine.changeOwnPassword(current: current, password: new)
            lastError = nil
            lastResult = "Password changed. Your other devices will have to sign in again."
            return true
        } catch {
            lastError = Self.describe(error)
            return false
        }
    }

    /// Engine errors carry a message worth reading; Swift's default rendering
    /// of them does not.
    func report(_ message: String) {
        lastError = message
    }

    static func describe(_ error: Error) -> String {
        switch error {
        case let KoanError.BadArgument(message): message
        case let KoanError.Database(message): message
        case let KoanError.NotFound(message): message
        case let KoanError.Audio(message): message
        default: error.localizedDescription
        }
    }
}

extension Notification.Name {
    /// Posted once a sign-out has gone through: what a television, whose
    /// signed-out state is a page of its own, waits on to show it.
    static let koanSignedOut = Notification.Name("koanSignedOut")
}
