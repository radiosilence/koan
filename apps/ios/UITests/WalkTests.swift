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
        // Silent, and blind to UPnP renderers: tests share a machine with its owner.
        app.launchEnvironment["KOAN_PLAYBACK__MUTED"] = "true"
        app.launchEnvironment["KOAN_PLAYBACK__RENDERERS"] = "false"
        app.launch()
    }

    func testWalk() {
        pause(3)
        // The launch sync's card floats over every page until it is done.
        // Waited out rather than polled: reading the screen while a launch is
        // still laying out can time out the test driver.
        pause(Double(ProcessInfo.processInfo.environment["KOAN_WALK_SETTLE"] ?? "0") ?? 0)
        snap("01-queue")

        tab("Library")
        snap("02-library")
        if open(app.buttons[any: "Albums"]) {
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
        if open(app.buttons[any: "Artists"]) {
            pause(3)
            snap("05-artists")
            back()
        }

        // Play whatever the queue holds, then open Now Playing from the mini
        // player that sits over the tab bar.
        let play = app.buttons[any: "play.fill"].firstMatch
        if play.waitForExistence(timeout: 3) { play.tap() }
        pause(4)
        // The mini player: above the tab bar on a phone, at the foot on an iPad.
        let pad = UIDevice.current.userInterfaceIdiom == .pad
        app.coordinate(withNormalizedOffset: CGVector(dx: pad ? 0.2 : 0.35, dy: pad ? 0.965 : 0.868)).tap()
        let lyrics = app.buttons[any: "Show lyrics"]
        if lyrics.waitForExistence(timeout: 5) {
            pause(2)
            snap("06-now-playing")
            lyrics.tap()
            pause(4)
            snap("07-lyrics")
            app.buttons[any: "Show artwork"].tap()
            app.swipeDown(velocity: .fast)
            pause(1)
        }

        tab("Settings")
        pause(1)
        snap("09-settings")
        if open(app.buttons.matching(NSPredicate(format: "label ==[c] %@", "Server")).firstMatch) {
            pause(1)
            app.swipeUp(velocity: .fast)
            app.swipeUp(velocity: .fast)
            pause(1)
            snap("09b-server-end")
            back()
        }
        if open(app.buttons.matching(NSPredicate(format: "label ==[c] %@", "Appearance")).firstMatch) {
            pause(1)
            snap("09c-appearance")
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
            field.typeText(ProcessInfo.processInfo.environment["KOAN_WALK_SEARCH"] ?? "gabriel")
            pause(3)
        }
        snap("08-search")
    }

    private func tab(_ name: String) {
        let button = app.tabBars.buttons[any: name]
        if button.waitForExistence(timeout: 3) { button.tap() } else { app.buttons[any: name].firstMatch.tap() }
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
