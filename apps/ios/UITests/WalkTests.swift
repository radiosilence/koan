import XCTest

/// Walks the app page by page and keeps a screenshot of each.
///
/// Not an assertion suite: the pages are SwiftUI over a live library, and what
/// is worth checking about them is how they look. `just ios-walk` runs this
/// against whatever library the simulator holds and exports the screenshots,
/// which is also how the App Store's are made.
@MainActor
final class WalkTests: XCTestCase {
    private var app: XCUIApplication!

    override func setUp() async throws {
        continueAfterFailure = true
        app = XCUIApplication()
        app.launch()
    }

    func testWalk() {
        pause(3)
        snap("01-queue")

        tab("Library")
        snap("02-library")
        if open(app.buttons["Albums"]) {
            pause(3)
            snap("03-albums")
            // The first sleeve in the grid, by where it sits: tiles are images
            // inside buttons inside a lazy grid, and none of that is stable
            // enough to query by.
            app.coordinate(withNormalizedOffset: CGVector(dx: 0.27, dy: 0.3)).tap()
            pause(3)
            snap("04-album")
            back()
            back()
        }
        if open(app.buttons["Artists"]) {
            pause(3)
            snap("05-artists")
            back()
        }

        // Play whatever the queue holds, then open Now Playing from the mini
        // player that sits over the tab bar.
        let play = app.buttons["play.fill"].firstMatch
        if play.waitForExistence(timeout: 3) { play.tap() }
        pause(4)
        app.coordinate(withNormalizedOffset: CGVector(dx: 0.35, dy: 0.868)).tap()
        let lyrics = app.buttons["Show lyrics"]
        if lyrics.waitForExistence(timeout: 5) {
            pause(2)
            snap("06-now-playing")
            lyrics.tap()
            pause(4)
            snap("07-lyrics")
            app.buttons["Show artwork"].tap()
            app.swipeDown(velocity: .fast)
            pause(1)
        }

        tab("Settings")
        pause(1)
        snap("09-settings")
        if open(app.buttons["Server"]) {
            pause(1)
            app.swipeUp(velocity: .fast)
            app.swipeUp(velocity: .fast)
            pause(1)
            snap("09b-server-end")
            back()
        }

        tab("Queue")
        pause(1)
        snap("10-queue-playing")

        // Last, because its keyboard covers the tab bar.
        tab("Search")
        // The search tab's field lives in the tab bar and arrives after the
        // tab does.
        let field = app.searchFields.firstMatch
        if field.waitForExistence(timeout: 8) {
            field.tap()
            field.typeText("gabriel")
            pause(3)
        }
        snap("08-search")
    }

    private func tab(_ name: String) {
        let button = app.tabBars.buttons[name]
        if button.waitForExistence(timeout: 3) { button.tap() } else { app.buttons[name].firstMatch.tap() }
        pause(1)
    }

    private func open(_ element: XCUIElement) -> Bool {
        guard element.waitForExistence(timeout: 3) else { return false }
        element.tap()
        return true
    }

    private func back() {
        let button = app.navigationBars.buttons.element(boundBy: 0)
        if button.exists { button.tap() }
        pause(1)
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
