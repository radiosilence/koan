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
        for (index, pane) in ["14-settings-server", "15-settings-playback", "16-settings-devices"].enumerated() {
            press(.down, times: index + 1)
            press(.select)
            pause(2)
            snap(pane)
            press(.menu)
            pause(1)
            press(.up, times: 6)
        }

        // The rest of the library, one section at a time.
        press(.left, times: 2)
        pause(1)
        for (index, section) in ["17-artists", "18-favourites", "19-playlists", "20-history"].enumerated() {
            press(.down, times: index + 2)
            press(.select)
            pause(4)
            snap(section)
            if index == 0 {
                press(.down)
                press(.select)
                pause(4)
                snap("21-artist")
                press(.menu)
                pause(1)
            }
            press(.menu)
            pause(1)
            press(.up, times: 8)
        }

        // A record's menu, held open from the album grid.
        press(.down)
        press(.select)
        pause(4)
        press(.down)
        remote.press(.select, forDuration: 1.5)
        pause(1.5)
        snap("22-album-menu")
        press(.menu)
        press(.menu)
        pause(1)

        // Lyrics and the device sheets, from Now Playing.
        press(.up, times: 8)
        press(.left, times: 4)
        pause(2)
        press(.down)
        press(.right, times: 4)
        press(.select)
        pause(3)
        snap("23-lyrics")
        press(.select)
        press(.right, times: 4)
        press(.select)
        pause(2)
        snap("24-devices-sheet")
        press(.menu)
        pause(1)
        press(.right)
        press(.select)
        pause(2)
        snap("25-output-sheet")
        press(.menu)
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
