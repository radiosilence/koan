import XCTest

/// Walks the television app with the remote, as someone on a sofa would, and
/// keeps a screenshot of each page.
///
/// Not an assertion suite, like the phone's walk: what is worth checking about
/// these pages is how they look and where focus goes. `just tv-walk` runs it
/// against the server named in the `KOAN_REMOTE__*` environment and exports the
/// screenshots.
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
        app.launch()
    }

    func testWalk() {
        pause(Double(ProcessInfo.processInfo.environment["KOAN_WALK_SETTLE"] ?? "8") ?? 8)
        snap("01-now-playing")

        // Up to the tab bar, then across it.
        press(.up)
        press(.right)
        pause(2)
        snap("02-queue")
        press(.down)
        snap("03-queue-focused")
        press(.select)
        pause(4)
        snap("04-queue-played")

        press(.up, times: 6)
        press(.left)
        pause(2)
        snap("05-now-playing-playing")
        press(.down)
        snap("06-now-playing-seek")
        press(.down)
        snap("07-now-playing-up-next")

        press(.up, times: 6)
        press(.right, times: 2)
        pause(2)
        snap("08-library")
        press(.down)
        press(.select)
        pause(4)
        snap("09-albums")
        press(.down)
        press(.select)
        pause(4)
        snap("10-album")
        press(.down)
        snap("11-album-track-focused")
        press(.menu)
        press(.menu)
        pause(1)

        press(.up, times: 6)
        press(.right)
        pause(2)
        snap("12-search")

        press(.up, times: 6)
        press(.right)
        pause(2)
        snap("13-settings")
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
