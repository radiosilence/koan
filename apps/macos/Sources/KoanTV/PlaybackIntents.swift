import AppIntents
import KoanFFI

/// Siri on the television: play, pause, and play a record from the library.
///
/// Each runs in the app's process, as an `AudioPlaybackIntent` so that tvOS
/// lets it start audio with the app in the background. An intent that
/// launches the app waits for the engine to come up rather than failing.
struct ResumeIntent: AudioPlaybackIntent {
    static let title: LocalizedStringResource = "Play"
    static let description = IntentDescription("Carries on playing what is in kōan's queue.")

    @MainActor
    func perform() async throws -> some IntentResult {
        await IntentTarget.app().player.resume()
        return .result()
    }
}

struct PauseIntent: AudioPlaybackIntent {
    static let title: LocalizedStringResource = "Pause"
    static let description = IntentDescription("Pauses kōan.")

    @MainActor
    func perform() async throws -> some IntentResult {
        await IntentTarget.app().player.pause()
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
    func perform() async throws -> some IntentResult {
        let app = await IntentTarget.app()
        guard let album = try await app.engine.album(albumId: Int64(record.id)) else {
            throw IntentError.notInLibrary(record.title)
        }
        let engine = app.engine
        await app.player.playNow(resolving: album.title) {
            await Playable.album(album).trackIds(using: engine)
        }.value
        return .result()
    }
}

enum IntentError: Error, CustomLocalizedStringResourceConvertible {
    case notInLibrary(String)

    var localizedStringResource: LocalizedStringResource {
        switch self {
        case .notInLibrary(let title): "\(title) is no longer in the library."
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
        let engine = await IntentTarget.app().engine
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
        let engine = await IntentTarget.app().engine
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
    private static var waiting: [CheckedContinuation<AppState, Never>] = []

    static func set(_ state: AppState) {
        self.state = state
        for w in waiting { w.resume(returning: state) }
        waiting = []
    }

    static func app() async -> AppState {
        if let state { return state }
        return await withCheckedContinuation { waiting.append($0) }
    }
}
