import AppKit
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

    init(engine: KoanEngine, activity: ActivityModel) {
        self.engine = engine
        self.activity = activity
        self.settings = engine.settings()
    }

    func reload() {
        settings = engine.settings()
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
        do {
            try engine.updateSettings(s: settings)
            lastError = nil
        } catch {
            lastError = Self.describe(error)
        }
    }

    // MARK: - Folders

    /// Ask for a folder and add it. Music lives in one place per person, so the
    /// panel starts at the last folder they picked rather than at the root.
    func addFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = true
        panel.prompt = "Add"
        panel.message = "Choose a folder to scan for music"
        guard panel.runModal() == .OK else { return }

        let added = panel.urls.map(\.path)
        edit { s in
            for path in added where !s.libraryFolders.contains(where: { $0.path == path }) {
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
                try engine.forgetFolder(path: path)
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
                try engine.scanReporting(force: force, reporter: progress)
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
                try engine.signInRemote(url: url, username: username, password: password)
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
        do {
            try engine.signOutRemote()
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
        let engine = self.engine
        Task {
            let result = await activity.run("Forgetting the server's tracks", exclusive: true) {
                try engine.forgetRemote()
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
            let result = await activity.run(full ? "Full sync with server" : "Syncing with server") {
                try engine.syncRemote(full: full)
            }
            switch result {
            case .success(let s):
                // A sync reconciles favourites too, so the hearts on screen are
                // out of date the moment it finishes.
                library?.refreshFavourites()
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
                try engine.clearDownloadCache()
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
                try engine.rebuildIndex()
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
