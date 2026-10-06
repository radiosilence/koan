import XCTest

/// A long page scrolls far enough that its last row clears the theme's tab
/// bar and mini player, which are drawn over the pages rather than beside
/// them. Opens an album from Search, as the report did, scrolls to its foot
/// and compares the last row with the bar.
///
/// Needs a signed-in simulator whose library has an album named by
/// `KOAN_BARINSET_ALBUM` (default "Thirty Rooms") with a last track named by
/// `KOAN_BARINSET_LAST` (default "Room 30"), long enough to scroll.
@MainActor
final class BarInsetTests: XCTestCase {
    private var app: XCUIApplication!

    override func setUp() async throws {
        continueAfterFailure = false
        addUIInterruptionMonitor(withDescription: "Notifications") { alert in
            alert.buttons.element(boundBy: 0).tap()
            return true
        }
        app = XCUIApplication()
        // Silent, and blind to UPnP renderers: tests share a machine with its owner.
        app.launchEnvironment["KOAN_PLAYBACK__MUTED"] = "true"
        app.launchEnvironment["KOAN_PLAYBACK__RENDERERS"] = "false"
        app.launch()
    }

    func testAlbumFromSearchScrollsClearOfTheBar() throws {
        let env = ProcessInfo.processInfo.environment
        let album = env["KOAN_BARINSET_ALBUM"] ?? "Thirty Rooms"
        let last = env["KOAN_BARINSET_LAST"] ?? "Room 30"

        app.buttons[any: "Search"].firstMatch.tap()
        let field = app.searchFields.firstMatch
        XCTAssertTrue(field.waitForExistence(timeout: 8))
        field.tap()
        field.typeText(album)
        let result = app.staticTexts[any: album]
        XCTAssertTrue(result.waitForExistence(timeout: 10), "no \(album) in the results")
        result.tap()

        let row = app.staticTexts[any: last]
        for _ in 0..<12 where !(row.exists && row.isHittable) {
            app.swipeUp()
        }
        // To the very foot: a list that misses the inset stops here with the
        // row still under the bar.
        app.swipeUp()
        app.swipeUp()
        pause(1)
        snap("album-foot")

        let bar = app.otherElements["koan-bar"]
        XCTAssertTrue(row.exists, "\(last) never appeared")
        if bar.exists {
            XCTAssertLessThanOrEqual(
                row.frame.maxY, bar.frame.minY + 1,
                "\(last) ends at \(row.frame.maxY), under the bar from \(bar.frame.minY)"
            )
        }
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
