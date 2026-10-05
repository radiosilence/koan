import XCTest

/// Walks the television app with the remote, as someone on a sofa would, and
/// keeps a screenshot of each page.
///
/// Not an assertion suite, like the phone's walk: what is worth checking about
/// these pages is how they look and where focus goes. `just tv-walk` runs it
/// against the server named in the `KOAN_REMOTE__*` environment and exports the
/// screenshots.
///
/// Each stop starts from a fresh launch, with focus on the first tab. Back at a
/// tab's root leaves the app on tvOS, so a walk that found its way home by
/// pressing it would end on the system's Home screen at the first misstep.
@MainActor
final class TVWalkTests: XCTestCase {
    private var app: XCUIApplication!
    private let remote = XCUIRemote.shared

    override func setUp() async throws {
        continueAfterFailure = true
        app = XCUIApplication()
        // The account, passed through from `just tv-walk`: the test runner
        // sees `TEST_RUNNER_KOAN_*` as `KOAN_*`, and the app sees nothing it
        // is not handed.
        for (key, value) in ProcessInfo.processInfo.environment where key.hasPrefix("KOAN_") {
            app.launchEnvironment[key] = value
        }
    }

    /// The tabs, left to right.
    private enum Tab: Int {
        case nowPlaying, queue, library, search, settings
    }

    func testWalk() {
        start(at: .nowPlaying)
        pause(Double(ProcessInfo.processInfo.environment["KOAN_WALK_SETTLE"] ?? "6") ?? 6)
        snap("01-now-playing")

        start(at: .queue)
        snap("02-queue")
        press(.down)
        snap("03-queue-focused")
        press(.select)
        pause(4)
        snap("04-queue-played")

        start(at: .nowPlaying)
        press(.down)
        snap("05-now-playing-controls")
        press(.up)
        snap("06-now-playing-seek")
        press(.down, times: 2)
        snap("07-now-playing-up-next")
        press(.up)
        press(.right, times: 4)
        press(.select)
        pause(3)
        snap("08-lyrics")
        press(.select)
        pause(1)
        press(.right, times: 4)
        press(.select)
        pause(2)
        snap("09-device-sheet")

        start(at: .library)
        snap("10-library")
        for (offset, name) in ["11-albums", "12-artists", "13-favourites", "14-playlists", "15-history"].enumerated() {
            start(at: .library)
            press(.down, times: offset + 1)
            press(.select)
            pause(4)
            snap(name)
            if offset == 0 {
                press(.down)
                press(.select)
                pause(4)
                snap("16-album")
                press(.down)
                snap("17-album-track")
                remote.press(.select, forDuration: 1.5)
                pause(1.5)
                snap("18-track-menu")
            }
            if offset == 1 {
                press(.down)
                press(.select)
                pause(4)
                snap("19-artist")
            }
        }

        start(at: .search)
        snap("20-search")

        start(at: .settings)
        snap("21-settings")
        for (offset, name) in ["22-settings-server", "23-settings-playback", "24-settings-devices"].enumerated() {
            start(at: .settings)
            press(.down, times: offset + 1)
            press(.select)
            pause(2)
            snap(name)
        }
    }

    /// Launch afresh and move along the tab bar to `tab`, then into its page.
    private func start(at tab: Tab) {
        app.terminate()
        app.launch()
        pause(5)
        press(.right, times: tab.rawValue)
        pause(2)
    }

    private func press(_ button: XCUIRemote.Button, times: Int = 1) {
        for _ in 0..<times {
            remote.press(button)
            pause(0.6)
        }
    }

    private func pause(_ seconds: Double) {
        Thread.sleep(forTimeInterval: seconds)
    }

    private func snap(_ name: String) {
        let shot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }
}
