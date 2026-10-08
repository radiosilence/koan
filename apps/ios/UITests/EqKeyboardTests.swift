import XCTest

/// Typing a band's figure on a tuning's page: the graph goes short, the row
/// stays between it and the keyboard, the bar above the keyboard steps
/// between figures, and Done keeps the value and puts the keyboard away.
@MainActor
final class EqKeyboardTests: XCTestCase {
    private var app: XCUIApplication!
    private var config: URL!

    override func setUp() async throws {
        continueAfterFailure = false
        addUIInterruptionMonitor(withDescription: "Notifications") { alert in
            alert.buttons.element(boundBy: 0).tap()
            return true
        }
        config = FileManager.default.temporaryDirectory.appending(path: "koan-eq-keyboard-\(UUID())")
        try FileManager.default.createDirectory(at: config, withIntermediateDirectories: true)
        // Enough bands that the last is well down the page.
        let bands = (0..<10)
            .map { i in "{ type = \"peaking\", freq = \(Int(40 * pow(2.0, Double(i)))).0, gain_db = 0.0, q = 1.0 }" }
            .joined(separator: ",\n    ")
        try """
        [[dsp.profiles]]
        name = "Test Tuning"
        role = "tuning"
        filters = [
            \(bands),
        ]
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

    func testTypingABand() throws {
        let settings = app.buttons[any: "Settings"]
        XCTAssert(settings.waitForExistence(timeout: 10), "no Settings tab")
        settings.tap()
        let eq = app.buttons[any: "EQ"]
        XCTAssert(eq.waitForExistence(timeout: 10), "no EQ in Settings")
        eq.tap()
        let manage = app.buttons[any: "Manage EQ"]
        reach(manage)
        manage.tap()
        let tuning = app.buttons.matching(NSPredicate(format: "label CONTAINS[c] 'Test Tuning'")).firstMatch
        reach(tuning)
        tuning.tap()

        let graph = app.descendants(matching: .any)[any: "EQ response"]
        XCTAssert(graph.waitForExistence(timeout: 10), "no graph")
        let tall = graph.frame.height

        let gain = app.textFields[any: "Band 10 gain"]
        reach(gain)
        gain.tap()
        let keyboard = app.keyboards.firstMatch
        XCTAssert(keyboard.waitForExistence(timeout: 5), "no keyboard")
        sleep(1)
        attach("typing")

        XCTAssertLessThan(graph.frame.height, tall - 50, "the graph kept its height")
        let done = app.buttons[any: "Done"]
        XCTAssert(done.exists, "no Done above the keyboard")
        XCTAssertGreaterThanOrEqual(gain.frame.minY, graph.frame.maxY, "the row is under the graph")
        XCTAssertLessThanOrEqual(gain.frame.maxY, done.frame.minY, "the row is under the keyboard's bar")
        XCTAssertGreaterThan(done.frame.minY, gain.frame.maxY, "Done is not with the keyboard")
        XCTAssertLessThanOrEqual(done.frame.maxY, keyboard.frame.minY + 1, "Done is not above the keyboard")

        gain.typeText(XCUIKeyboardKey.delete.rawValue + XCUIKeyboardKey.delete.rawValue + "3")

        app.buttons[any: "Previous"].tap()
        XCTAssert(hasFocus(app.textFields[any: "Band 10 frequency"]), "Previous did not reach the frequency")
        app.buttons[any: "Next"].tap()
        XCTAssert(hasFocus(app.textFields[any: "Band 10 gain"]), "Next did not reach the gain")
        attach("next")
        app.buttons[any: "Next"].tap()
        XCTAssert(hasFocus(app.textFields[any: "Band 10 Q"]), "Next did not reach Q")
        app.buttons[any: "Next"].tap()
        XCTAssert(hasFocus(app.textFields[any: "Band 10 Q"]), "Next past the last figure")

        done.tap()
        XCTAssert(keyboard.waitForNonExistence(timeout: 5), "Done left the keyboard up")
        sleep(1)
        attach("done")
        XCTAssertEqual(graph.frame.height, tall, accuracy: 1, "the graph did not come back")
        try waitForSaved("gain_db = 3")
    }

    /// Whether `field` takes the keyboard within a couple of seconds: moving
    /// focus commits the figure left, and the rows are redrawn from it.
    private func hasFocus(_ field: XCUIElement) -> Bool {
        let focused = NSPredicate { element, _ in
            ((element as? XCUIElement)?.value(forKey: "hasKeyboardFocus") as? Bool) ?? false
        }
        return XCTWaiter().wait(for: [expectation(for: focused, evaluatedWith: field)], timeout: 3) == .completed
    }

    /// Both layers: the first save moves the tuning to `config.local.toml`.
    private func saved() throws -> String {
        try ["config.toml", "config.local.toml"]
            .map { config.appending(path: $0) }
            .filter { FileManager.default.fileExists(atPath: $0.path) }
            .map { try String(contentsOf: $0, encoding: .utf8) }
            .joined(separator: "\n")
    }

    private func waitForSaved(_ text: String) throws {
        for _ in 0..<20 {
            if try saved().contains(text) { return }
            usleep(250_000)
        }
        XCTFail("never saved \(text): \(try saved())")
    }

    private func attach(_ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }

    /// Scrolls until `element` is clear of the mini player and tab bar,
    /// from the right edge, which no chart or slider takes.
    private func reach(_ element: XCUIElement) {
        for _ in 0..<10 where !(element.exists && element.isHittable && element.frame.maxY < 700) {
            app.coordinate(withNormalizedOffset: CGVector(dx: 0.95, dy: 0.75))
                .press(forDuration: 0.05, thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.95, dy: 0.35)))
        }
        XCTAssert(element.waitForExistence(timeout: 5), "\(element) never appeared")
    }
}
