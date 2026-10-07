import XCTest

/// Select mode on an album page, the album grid, the queue and a shelf: ticking rows
/// and tiles, the bar's verbs, and done. Screenshots of each step, for how it
/// looks; assertions for what VoiceOver finds.
@MainActor
final class SelectTests: XCTestCase {
    private var app: XCUIApplication!

    override func setUp() async throws {
        continueAfterFailure = true
        app = XCUIApplication()
        app.launchEnvironment["KOAN_PLAYBACK__MUTED"] = "true"
        app.launchEnvironment["KOAN_PLAYBACK__RENDERERS"] = "false"
        for (key, value) in ProcessInfo.processInfo.environment where key.hasPrefix("KOAN_") {
            app.launchEnvironment[key] = value
        }
        app.launchArguments += ["-graphics", "1"]
        app.launch()
    }

    func testSelect() {
        pause(3)
        snap("01-queue")

        app.buttons[any: "Library"].tap()
        pause(1)
        snap("02-library")
        app.buttons[any: "Albums"].tap()
        pause(3)
        XCTAssertTrue(app.buttons[any: "Select"].waitForExistence(timeout: 3), "albums offer select")
        app.buttons[any: "Select"].tap()
        pause(1)
        app.coordinate(withNormalizedOffset: CGVector(dx: 0.27, dy: 0.3)).tap()
        app.coordinate(withNormalizedOffset: CGVector(dx: 0.73, dy: 0.3)).tap()
        pause(1)
        snap("03-albums-selecting")
        XCTAssertTrue(app.staticTexts[any: "2 selected"].waitForExistence(timeout: 3), "two records ticked")
        app.buttons[any: "Done"].tap()
        pause(1)

        app.coordinate(withNormalizedOffset: CGVector(dx: 0.27, dy: 0.3)).tap()
        pause(3)
        snap("04-album")
        app.buttons[any: "Select"].tap()
        pause(1)
        let rows = app.cells
        if rows.count > 3 {
            rows.element(boundBy: 2).tap()
            rows.element(boundBy: 3).tap()
        }
        pause(1)
        snap("05-album-selecting")
        XCTAssertTrue(app.staticTexts[any: "2 selected"].exists, "two tracks ticked")
        for verb in ["Play", "Play Next", "Add to Queue", "Add to Playlist", "Favourite"] {
            XCTAssertTrue(app.buttons[any: verb].exists, "\(verb) in the bar")
        }
        app.buttons[any: "Play"].tap()
        pause(3)

        app.buttons[any: "Queue"].tap()
        pause(2)
        snap("06-queue-playing")
        app.buttons[any: "Select"].tap()
        pause(1)
        let queued = app.cells
        if queued.count > 1 {
            queued.element(boundBy: 0).tap()
            queued.element(boundBy: 1).tap()
        }
        pause(1)
        snap("07-queue-selecting")
        XCTAssertTrue(app.buttons[any: "Remove"].exists, "the queue's pick can be removed")
        XCTAssertFalse(app.buttons[any: "Play Next"].exists, "the queue's rows are already queued")
        app.buttons[any: "Done"].tap()
        pause(1)
        snap("08-queue-done")

        // Settings pushes its panes by link; chosen again, its tab still
        // goes back to the list of them.
        app.buttons[any: "Settings"].tap()
        pause(1)
        app.buttons[any: "Appearance"].tap()
        pause(1)
        snap("09a-settings-pane")
        app.buttons[any: "Settings"].tap()
        pause(1)
        XCTAssertTrue(app.buttons[any: "Appearance"].waitForExistence(timeout: 3), "back at the settings root")
        snap("09b-settings-root")
        app.swipeUp(velocity: .fast)
        app.swipeUp(velocity: .fast)
        pause(1)
        snap("09c-settings-end")

        // Chosen again, the tab goes back to its root.
        app.buttons[any: "Library"].tap()
        pause(1)
        app.buttons[any: "Library"].tap()
        pause(1)
        app.buttons[any: "Favourites"].tap()
        pause(3)
        snap("09-favourites")

        // A shelf picks across its artists, records and tracks, as search does.
        XCTAssertTrue(app.buttons[any: "Select"].waitForExistence(timeout: 3), "the shelf offers select")
        app.buttons[any: "Select"].tap()
        pause(1)
        let tracks = app.descendants(matching: .any).matching(NSPredicate(format: "identifier BEGINSWITH 'track-'"))
        if tracks.count > 1 {
            tracks.element(boundBy: 0).tap()
            tracks.element(boundBy: 1).tap()
        }
        pause(1)
        snap("10-favourites-selecting")
        XCTAssertTrue(app.staticTexts[any: "2 selected"].exists, "two shelf tracks ticked")
        for verb in ["Play", "Play Next", "Add to Queue", "Add to Playlist", "Favourite"] {
            XCTAssertTrue(app.buttons[any: verb].exists, "\(verb) in the shelf's bar")
        }
        app.buttons[any: "Done"].tap()
        pause(1)
        XCTAssertTrue(app.buttons[any: "Select"].exists, "done ends the shelf's select mode")
    }

    private func pause(_ seconds: TimeInterval) {
        _ = XCTWaiter.wait(for: [expectation(description: "settle")], timeout: seconds)
    }

    private func snap(_ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }
}
