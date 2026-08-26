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

    /// A sync changes favourites and the library listing, and the browser is
    /// the thing that has to show it. Weak so the settings window does not keep
    /// the browse state alive.
    weak var library: LibraryModel?

    private(set) var settings: Settings
    private(set) var lastError: String?
    private(set) var lastResult: String?

    /// Typed here rather than in `settings`, because it never comes back out of
    /// the engine — the credential store is write-only from this side.
    var password = ""

    init(engine: KoanEngine, activity: ActivityModel) async {
        self.engine = engine
        self.activity = activity
        self.settings = await engine.settings()
    }

    func reload() {
        Task { settings = await engine.settings() }
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
            let result = await activity.run("Forgetting \(URL(fileURLWithPath: path).lastPathComponent)", exclusive: true) {
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
                force ? "Rescanning every file" : "Scanning library"
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
        Task {
            let result = await activity.run("Signing in") {
                try await engine.signInRemote(url: url, username: username, password: password)
            }
            switch result {
            case .success:
                self.password = ""
                lastError = nil
                lastResult = "Signed in to \(url)"
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
            } catch {
                lastError = Self.describe(error)
                reload()
                return
            }
            guard forgetTracks else {
                reload()
                return
            }
            let result = await activity.run("Forgetting the server's tracks", exclusive: true) {
                try await self.engine.forgetRemote()
            }
            switch result {
            case .success(let n): lastResult = "Signed out and forgot \(n.formatted(.number)) tracks"
            case .failure(let e): lastError = Self.describe(e)
            }
            reload()
        }
    }

    func syncNow(full: Bool) {
        let engine = self.engine
        Task {
            // `runReporting` rather than `run`: the counts exist while the sync
            // is running and used to be visible only in the log, so the row said
            // "Syncing with server" for a minute and nothing more.
            let result = await activity.runReporting(
                full ? "Full sync with server" : "Syncing with server"
            ) { progress in
                try await engine.syncRemoteReporting(full: full, reporter: progress)
            }
            switch result {
            case .success(let s):
                // A sync reconciles favourites too, so the hearts on screen are
                // out of date the moment it finishes.
                library?.refreshFavourites()
                // And the library itself: a sync that wrote thousands of rows
                // has changed what there is to show.
                library?.loadInitial()
                // Zero is the normal answer for an incremental sync with nothing
                // new, and "0 tracks across 0 albums" reads as a failure.
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
            let result = await activity.run("Clearing downloads") {
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
            let result = await activity.run("Clearing the library index") {
                try await engine.rebuildIndex()
            }
            switch result {
            case .success(let s):
                lastResult = "Removed \(s.tracks) tracks — scan or sync to rebuild"
            case .failure(let e):
                lastError = Self.describe(e)
            }
            reload()
        }
    }

    /// Engine errors carry a message worth reading; Swift's default rendering
    /// of them does not.
    private static func describe(_ error: Error) -> String {
        switch error {
        case let KoanError.BadArgument(message): message
        case let KoanError.Database(message): message
        case let KoanError.NotFound(message): message
        case let KoanError.Audio(message): message
        default: error.localizedDescription
        }
    }
}
