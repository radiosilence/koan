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

        // Something playing, for the pages about what is playing: the first
        // record in the grid, from its Play button.
        start(at: .library)
        press(.down)
        press(.select)
        pause(4)
        press(.down)
        press(.select)
        pause(4)
        if focus(app.buttons["Play"]) {
            press(.select)
            pause(6)
        }

        start(at: .queue)
        snap("02-queue")
        press(.down)
        snap("03-queue-focused")
        press(.select)
        pause(4)
        snap("04-queue-played")

        // Down from the tab bar lands on play/pause.
        start(at: .nowPlaying)
        press(.down)
        snap("05-now-playing-controls")
        press(.up)
        snap("06-now-playing-seek")
        press(.down, times: 2)
        snap("07-now-playing-up-next")
        start(at: .nowPlaying)
        press(.down)
        if focus(app.buttons["lyrics"]) {
            press(.select)
            pause(3)
        }
        snap("08-lyrics")
        start(at: .nowPlaying)
        press(.down)
        if focus(app.buttons["output"]) || focus(app.buttons["play-on"]) {
            snap("09-devices-focused")
            press(.select)
            pause(2)
        }
        snap("10-device-sheet")

        start(at: .library)
        snap("11-library")
        for (offset, name) in ["12-albums", "13-artists", "14-favourites", "15-playlists", "16-history"].enumerated() {
            start(at: .library)
            press(.down, times: offset + 1)
            press(.select)
            pause(4)
            snap(name)
            if offset == 0 {
                press(.down)
                press(.select)
                pause(4)
                snap("17-album")
                press(.down)
                snap("18-album-track")
                remote.press(.select, forDuration: 1.5)
                pause(1.5)
                snap("19-track-menu")
            }
            if offset == 1 {
                press(.down)
                press(.select)
                pause(4)
                snap("20-artist")
            }
        }

        start(at: .search)
        snap("21-search")

        start(at: .settings)
        snap("22-settings")
        for (offset, name) in ["23-settings-server", "24-settings-playback", "25-settings-devices"].enumerated() {
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

    /// Move until `element` has focus: along the row, then down and up the
    /// page. Counting presses breaks whenever a control comes or goes.
    @discardableResult
    private func focus(_ element: XCUIElement) -> Bool {
        guard element.waitForExistence(timeout: 3) else { return false }
        for direction in [XCUIRemote.Button.right, .left, .up, .down] {
            for _ in 0..<10 {
                if element.hasFocus { return true }
                press(direction)
            }
        }
        return element.hasFocus
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
