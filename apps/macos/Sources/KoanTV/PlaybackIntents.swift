import AppIntents
import KoanFFI

/// Siri on the television: play, pause, and play a record from the library.
///
/// Each runs in the app's process, as an `AudioPlaybackIntent` so that tvOS
/// lets it start audio with the app in the background. An intent that
/// launches the app waits for the engine to come up, for a while. They call
/// the engine rather than `PlayerModel`, whose transport keeps errors for the
/// screen: what goes wrong here has to be said back to whoever asked.
struct ResumeIntent: AudioPlaybackIntent {
    static let title: LocalizedStringResource = "Play"
    static let description = IntentDescription("Carries on playing what is in kōan's queue.")

    @MainActor
    func perform() async throws -> some IntentResult {
        let app = try await IntentTarget.ready()
        guard !app.player.queue.isEmpty else { throw IntentError.emptyQueue }
        try await IntentError.saying { try await app.engine.resume() }
        return .result()
    }
}

struct PauseIntent: AudioPlaybackIntent {
    static let title: LocalizedStringResource = "Pause"
    static let description = IntentDescription("Pauses kōan.")

    @MainActor
    func perform() async throws -> some IntentResult {
        let app = try await IntentTarget.app()
        try await IntentError.saying { try await app.engine.pause() }
        return .result()
    }
}

struct PlayRecordIntent: AudioPlaybackIntent {
    static let title: LocalizedStringResource = "Play a Record"
    static let description = IntentDescription("Plays a record from your library, from its first track.")

    @Parameter(title: "Record") var record: RecordEntity

    init() {}

    init(record: RecordEntity) {
        self.record = record
    }

    @MainActor
    func perform() async throws -> some IntentResult & ProvidesDialog {
        let app = try await IntentTarget.ready()
        guard let album = try await app.engine.album(albumId: Int64(record.id)) else {
            throw IntentError.notInLibrary(record.title)
        }
        // While the server is out of reach, only what is downloaded can play.
        let offline = app.mirror.connection?.offline == true
        let tracks = try await app.engine.tracks(albumId: album.id, artistId: nil, sort: .album, limit: 2000, offset: 0)
        let ids = tracks.filter { !offline || $0.onDisk }.map(\.id)
        guard !ids.isEmpty else {
            throw offline ? IntentError.notDownloaded(album.title) : IntentError.nothingToPlay(album.title)
        }
        try await IntentError.saying { _ = try await app.engine.replaceQueue(trackIds: ids, startAt: 0) }
        return .result(dialog: "Playing \(album.title) by \(album.artistName).")
    }
}

/// What Siri says when an intent cannot do what was asked.
enum IntentError: Error, CustomLocalizedStringResourceConvertible {
    case notReady
    case startFailed(String)
    case signedOut
    case emptyQueue
    case notInLibrary(String)
    case notDownloaded(String)
    case nothingToPlay(String)
    case engine(String)

    var localizedStringResource: LocalizedStringResource {
        switch self {
        case .notReady: "kōan is still starting. Try again in a moment."
        case .startFailed(let why): "kōan could not start: \(why)"
        case .signedOut: "Sign in to kōan on the Apple TV first."
        case .emptyQueue: "Nothing is queued in kōan. Ask it to play a record."
        case .notInLibrary(let title): "\(title) is no longer in the library."
        case .notDownloaded(let title): "\(title) isn't downloaded, and the server can't be reached."
        case .nothingToPlay(let title): "\(title) has nothing that can play here."
        case .engine(let why): "kōan couldn't do that: \(why)"
        }
    }

    /// Runs an engine call, with its error put in words Siri can say.
    @MainActor
    static func saying(_ call: () async throws -> Void) async throws {
        do {
            try await call()
        } catch {
            throw IntentError.engine(String(describing: error))
        }
    }
}

/// A record, as Siri names it: its title and artist.
struct RecordEntity: AppEntity {
    static let typeDisplayRepresentation: TypeDisplayRepresentation = "Record"
    static let defaultQuery = RecordQuery()

    /// The album's row id; `Int` because entity ids cannot be `Int64`.
    let id: Int
    let title: String
    let artist: String

    var displayRepresentation: DisplayRepresentation {
        DisplayRepresentation(title: "\(title)", subtitle: "\(artist)")
    }
}

/// Records by the library's fuzzy match, the one the search page uses.
struct RecordQuery: EntityStringQuery {
    @MainActor
    func entities(for identifiers: [Int]) async throws -> [RecordEntity] {
        let engine = try await IntentTarget.ready().engine
        var found: [RecordEntity] = []
        for id in identifiers {
            if let album = try await engine.album(albumId: Int64(id)) {
                found.append(RecordEntity(id: Int(album.id), title: album.title, artist: album.artistName))
            }
        }
        return found
    }

    @MainActor
    func entities(matching string: String) async throws -> [RecordEntity] {
        let engine = try await IntentTarget.ready().engine
        let matches = try await engine.fuzzySearch(query: string, kind: .album, limit: 10)
        return try await entities(for: matches.map { Int($0.id) })
    }
}

struct KoanShortcuts: AppShortcutsProvider {
    static var appShortcuts: [AppShortcut] {
        AppShortcut(
            intent: PlayRecordIntent(),
            phrases: ["Play a record in \(.applicationName)"],
            shortTitle: "Play a Record",
            systemImageName: "square.stack"
        )
        AppShortcut(
            intent: ResumeIntent(),
            phrases: ["Play \(.applicationName)", "Resume \(.applicationName)"],
            shortTitle: "Play",
            systemImageName: "play.fill"
        )
        AppShortcut(
            intent: PauseIntent(),
            phrases: ["Pause \(.applicationName)"],
            shortTitle: "Pause",
            systemImageName: "pause.fill"
        )
    }
}

/// The running app, handed over once its engine is up.
@MainActor
enum IntentTarget {
    private static var state: AppState?
    private static var failure: String?
    private static var waiting: [UUID: CheckedContinuation<AppState, any Error>] = [:]
    /// Long enough for a cold launch to open the database; past it, Siri is
    /// told to try again rather than left waiting.
    private static let patience: Duration = .seconds(20)

    static func set(_ state: AppState) {
        self.state = state
        for w in waiting.values { w.resume(returning: state) }
        waiting = [:]
    }

    /// The app could not start, so no intent can run.
    static func fail(_ why: String) {
        failure = why
        for w in waiting.values { w.resume(throwing: IntentError.startFailed(why)) }
        waiting = [:]
    }

    static func app() async throws -> AppState {
        if let state { return state }
        if let failure { throw IntentError.startFailed(failure) }
        let id = UUID()
        return try await withCheckedThrowingContinuation { continuation in
            waiting[id] = continuation
            Task { @MainActor in
                try? await Task.sleep(for: patience)
                waiting.removeValue(forKey: id)?.resume(throwing: IntentError.notReady)
            }
        }
    }

    /// The app, signed in to a server: what playing anything needs.
    static func ready() async throws -> AppState {
        let app = try await app()
        guard await app.engine.settings().remoteSignedIn else { throw IntentError.signedOut }
        return app
    }
}
