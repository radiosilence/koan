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
        // Silent, and blind to UPnP renderers: tests share a machine with its owner.
        app.launchEnvironment["KOAN_PLAYBACK__MUTED"] = "true"
        app.launchEnvironment["KOAN_PLAYBACK__RENDERERS"] = "false"
        // The account, passed through from `just tv-walk`: the test runner
        // sees `TEST_RUNNER_KOAN_*` as `KOAN_*`, and the app sees nothing it
        // is not handed.
        for (key, value) in ProcessInfo.processInfo.environment where key.hasPrefix("KOAN_") {
            app.launchEnvironment[key] = value
        }
        // The walk relaunches the app at every stop; on the network, each
        // launch would announce it to, and dial, every device in the house.
        app.launchEnvironment["KOAN_DEVICES__NEARBY"] = "false"
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
        // record in the grid, from its first track, where opening it leaves
        // focus.
        start(at: .library)
        press(.down)
        press(.select)
        pause(4)
        press(.down)
        press(.select)
        pause(4)
        press(.select)
        pause(8)

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
        press(.down)
        snap("06-now-playing-seek")
        press(.down)
        snap("07-now-playing-up-next")
        start(at: .nowPlaying)
        press(.down)
        if focus(app.buttons[any: "lyrics"]) {
            press(.select)
            pause(3)
        }
        snap("08-lyrics")
        start(at: .nowPlaying)
        press(.down)
        if focus(app.buttons[any: "output"]) || focus(app.buttons[any: "play-on"]) {
            snap("09-devices-focused")
            press(.select)
            pause(2)
        }
        snap("10-device-sheet")

        start(at: .library)
        snap("11-library")
        // Each page by its place in the library index, counted from the tab bar.
        let pages = [
            ("12-albums", 1), ("13-artists", 2), ("14-favourites", 3),
            ("15-playlists", 4), ("15b-recently-played", 5), ("15c-downloaded", 6), ("16-history", 7),
        ]
        for (name, place) in pages {
            start(at: .library)
            press(.down, times: place)
            press(.select)
            pause(4)
            snap(name)
            if place == 1 {
                // The browser's own controls sit above the listing, and each
                // must open something: in a toolbar they took focus and did
                // nothing. Focus arrives on the filter field, beside them.
                press(.right)
                snap("12a-controls")
                press(.select)
                pause(2)
                snap("12b-control-opened")
                let opened = app.switches[any: "Favourites"].exists
                    || app.buttons[any: "Recently Added"].exists || app.buttons[any: "Artist"].exists
                XCTAssertTrue(opened, "a browser control opens its filters or its sort")
                press(.menu)
                pause(1)
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
            if place == 2 {
                press(.down)
                press(.select)
                pause(4)
                snap("20-artist")
            }
            if place == 5 {
                // `KOAN_WALK_PLAYLIST` names one, on a server with many.
                let wanted = ProcessInfo.processInfo.environment["KOAN_WALK_PLAYLIST"]
                    .map { app.buttons.containing(NSPredicate(format: "label CONTAINS %@", $0)).firstMatch }
                if let wanted, focus(wanted) {
                    press(.select)
                } else {
                    press(.down)
                    press(.select)
                }
                pause(4)
                snap("15d-playlist")
            }
        }

        start(at: .search)
        snap("21-search")
        press(.down)
        let field = app.searchFields.firstMatch
        if field.waitForExistence(timeout: 3) {
            field.typeText(ProcessInfo.processInfo.environment["KOAN_WALK_SEARCH"] ?? "bliss")
            pause(4)
            snap("21b-search-results")
            press(.down, times: 2)
            snap("21c-search-focused")
        }

        start(at: .settings)
        snap("22-settings")
        for (offset, name) in ["23-settings-server", "24-settings-playback", "25-settings-eq", "26-settings-devices", "27-settings-appearance"].enumerated() {
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
