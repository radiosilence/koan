import XCTest

/// Picking the target a tuning was made against ticks it and closes the list,
/// before the save behind it has finished.
@MainActor
final class MadeAgainstTests: XCTestCase {
    private var app: XCUIApplication!
    private var config: URL!

    override func setUp() async throws {
        continueAfterFailure = false
        addUIInterruptionMonitor(withDescription: "Notifications") { alert in
            alert.buttons.element(boundBy: 0).tap()
            return true
        }
        // A configuration of its own, holding one tuning: the simulator's
        // runner and app share the host's file system.
        config = FileManager.default.temporaryDirectory.appending(path: "koan-made-against-\(UUID())")
        try FileManager.default.createDirectory(at: config, withIntermediateDirectories: true)
        try """
        [[dsp.profiles]]
        name = "Test Tuning"
        role = "tuning"
        filters = [{ type = "low_shelf", freq = 105.0, gain_db = 3.0, q = 0.7 }]
        """.write(to: config.appending(path: "config.toml"), atomically: true, encoding: .utf8)
        app = XCUIApplication()
        app.launchEnvironment["KOAN_CONFIG_DIR"] = config.path
        app.launchEnvironment["KOAN_PLAYBACK__MUTED"] = "true"
        app.launchEnvironment["KOAN_PLAYBACK__RENDERERS"] = "false"
        app.launch()
    }

    override func tearDown() async throws {
        app.terminate()
        try? FileManager.default.removeItem(at: config)
    }

    func testPickStaysPicked() throws {
        let settings = app.tabBars.buttons[any: "Settings"]
        if settings.waitForExistence(timeout: 10) {
            settings.tap()
        } else {
            app.buttons[any: "Settings"].firstMatch.tap()
        }
        let eq = app.buttons[any: "EQ"]
        XCTAssert(eq.waitForExistence(timeout: 10), "no EQ in Settings")
        eq.tap()
        let manage = app.buttons[any: "Manage EQ"]
        reach(manage)
        manage.tap()
        let tuning = app.buttons.matching(NSPredicate(format: "label CONTAINS[c] 'Test Tuning'")).firstMatch
        reach(tuning)
        tuning.tap()

        let made = app.buttons.matching(NSPredicate(format: "label ==[c] 'Made against'")).firstMatch
        reach(made)
        XCTAssertEqual(made.value as? String, "Unknown")
        made.tap()
        let harman = app.buttons.matching(NSPredicate(format: "label BEGINSWITH[c] 'Harman in-ear 2019'")).firstMatch
        reach(harman)
        harman.tap()

        // The list closes on the page, which says the pick.
        XCTAssert(made.waitForExistence(timeout: 5), "the list stayed open")
        XCTAssert((made.value as? String)?.hasPrefix("Harman in-ear 2019") == true, "Made against says \(made.value ?? "nothing")")
    }

    /// Scrolls until `element` is clear of the mini player and tab bar,
    /// from the right edge, which no chart or slider takes.
    private func reach(_ element: XCUIElement) {
        for _ in 0..<8 where !(element.exists && element.frame.maxY < 700) {
            app.coordinate(withNormalizedOffset: CGVector(dx: 0.95, dy: 0.75))
                .press(forDuration: 0.05, thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.95, dy: 0.35)))
        }
        XCTAssert(element.waitForExistence(timeout: 5), "\(element) never appeared")
    }
}
