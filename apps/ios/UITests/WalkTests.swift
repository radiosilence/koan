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
        addUIInterruptionMonitor(withDescription: "Notifications") { alert in
            alert.buttons.element(boundBy: 0).tap()
            return true
        }
        app = XCUIApplication()
        // Silent, and blind to UPnP renderers: tests share a machine with its owner.
        app.launchEnvironment["KOAN_PLAYBACK__MUTED"] = "true"
        app.launchEnvironment["KOAN_PLAYBACK__RENDERERS"] = "false"
        // Settings passed through from `just ios-walk`, such as the theme: the
        // test runner sees `TEST_RUNNER_KOAN_*` as `KOAN_*`.
        for (key, value) in ProcessInfo.processInfo.environment where key.hasPrefix("KOAN_") {
            app.launchEnvironment[key] = value
        }
        // The wash held still: drifting, it keeps the app from ever being
        // idle, and the test driver waits on that. A still frame of either
        // looks the same.
        app.launchArguments += ["-graphics", "1"]
        app.launch()
    }

    func testWalk() {
        pause(3)
        // The launch sync's card floats over every page until it is done.
        // Waited out rather than polled: reading the screen while a launch is
        // still laying out can time out the test driver.
        pause(Double(ProcessInfo.processInfo.environment["KOAN_WALK_SETTLE"] ?? "0") ?? 0)
        snap("01-queue")

        // An iPad has no Library tab: its pages are rows of the sidebar, and
        // the detail column's root has no back button to return to.
        let pad = UIDevice.current.userInterfaceIdiom == .pad
        if !pad {
            tab("Library")
            snap("02-library")
        }
        if open(page("Albums")) {
            pause(3)
            snap("03-albums")
            // The first sleeve in the grid, by where it sits: tiles are images
            // inside buttons inside a lazy grid, and none of that is stable
            // enough to query by.
            app.coordinate(withNormalizedOffset: CGVector(dx: pad ? 0.42 : 0.27, dy: 0.3)).tap()
            pause(3)
            snap("04-album")
            back()
            if !pad { back() }
        }
        if open(page("Artists")) {
            pause(3)
            snap("05-artists")
            if !pad { back() }
        }

        // Play whatever the queue holds, then open Now Playing from the mini
        // player that sits over the tab bar.
        let play = app.buttons[any: "play.fill"].firstMatch
        if play.waitForExistence(timeout: 3) { play.tap() }
        pause(4)
        // The mini player: above the tab bar on a phone, at the foot of the
        // detail column on an iPad.
        app.coordinate(withNormalizedOffset: CGVector(dx: pad ? 0.5 : 0.35, dy: pad ? 0.965 : 0.868)).tap()
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
        // The server's sections, where the server and the account have them.
        for (name, shot) in [
            ("Account", "09d-account"), ("People", "09e-people"),
            ("Devices", "09f-devices"), ("Integrations", "09g-server-capabilities"),
            ("Appearance", "09c-appearance"),
        ] {
            if open(app.buttons.matching(NSPredicate(format: "label ==[c] %@", name)).firstMatch) {
                pause(1)
                snap(shot)
                back()
            }
        }

        tab("Queue")
        pause(1)
        snap("10-queue-playing")

        // Last, because its keyboard covers the tab bar.
        tab("Search")
        // The search tab's field lives in the tab bar and arrives after the
        // tab does. The kōan look's is a text field under the title.
        var field = app.searchFields.firstMatch
        if !field.waitForExistence(timeout: 8) {
            field = app.textFields[any: "Artists, albums, tracks"]
        }
        if field.exists {
            field.tap()
            field.typeText(ProcessInfo.processInfo.environment["KOAN_WALK_SEARCH"] ?? "gabriel")
            pause(3)
        }
        snap("08-search")
    }

    private func tab(_ name: String) {
        let button = app.tabBars.buttons[any: name]
        if button.waitForExistence(timeout: 3) {
            button.tap()
        } else if app.buttons[any: name].exists {
            app.buttons[any: name].tap()
        } else {
            // An iPad's sidebar: rows whose label is their text.
            app.staticTexts[any: name].tap()
        }
        pause(1)
    }

    /// A library page: a row of the phone's Library tab, or of an iPad's sidebar.
    private func page(_ name: String) -> XCUIElement {
        let button = app.buttons[any: name]
        return button.exists ? button : app.staticTexts[any: name]
    }

    private func open(_ element: XCUIElement) -> Bool {
        guard element.waitForExistence(timeout: 3) else { return false }
        element.tap()
        return true
    }

    private func back() {
        // The kōan look draws its own back button, which is not always the
        // bar's first.
        let drawn = app.navigationBars.buttons
            .matching(NSPredicate(format: "label IN %@", ["Back", "chevron.left"])).firstMatch
        let button = drawn.exists ? drawn : app.navigationBars.buttons.element(boundBy: 0)
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
