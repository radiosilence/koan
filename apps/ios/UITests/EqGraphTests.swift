import XCTest

/// Editing a tuning on its graph: a swipe up or down that starts on the graph
/// scrolls the page and edits nothing, a stroke across it paints the curve, a
/// band's handle drags, and a pinch changes the band's Q. Each edit is read
/// back from the configuration the app saves.
@MainActor
final class EqGraphTests: XCTestCase {
    private var app: XCUIApplication!
    private var config: URL!

    override func setUp() async throws {
        continueAfterFailure = false
        addUIInterruptionMonitor(withDescription: "Notifications") { alert in
            alert.buttons.element(boundBy: 0).tap()
            return true
        }
        config = FileManager.default.temporaryDirectory.appending(path: "koan-eq-graph-\(UUID())")
        try FileManager.default.createDirectory(at: config, withIntermediateDirectories: true)
        let points = (0..<20)
            .map { i in "[\(20 * pow(1.44, Double(i))), 0.0]" }
            .joined(separator: ", ")
        try """
        [[dsp.profiles]]
        name = "Test Tuning"
        role = "tuning"
        filters = [
            { type = "peaking", freq = 1000.0, gain_db = 0.0, q = 1.0 },
            { type = "graphic", points = [\(points)] },
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

    func testGraphEdits() throws {
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

        let graph = app.descendants(matching: .any)[any: "EQ response"]
        XCTAssert(graph.waitForExistence(timeout: 10), "no graph")
        sleep(1)
        attach("page")
        let before = try saved()

        // Up from the graph's left, where no handle is: the page scrolls.
        let top = graph.frame.minY
        point(graph, 0.15, 0.8).press(forDuration: 0.05, thenDragTo: point(graph, 0.15, 0.05))
        sleep(1)
        attach("after vertical swipe")
        XCTAssertLessThan(graph.frame.minY, top - 20, "the page did not scroll")
        XCTAssertEqual(try saved(), before, "a vertical swipe edited the EQ")
        // Back down, from the right edge, which no chart takes.
        for _ in 0..<3 {
            app.coordinate(withNormalizedOffset: CGVector(dx: 0.95, dy: 0.3))
                .press(forDuration: 0.05, thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.95, dy: 0.8)))
        }
        sleep(1)

        // Across the graph's low end, near the top: the curve is painted up.
        point(graph, 0.15, 0.2).press(forDuration: 0.05, thenDragTo: point(graph, 0.35, 0.2))
        try waitForSave(from: before, "a stroke across the graph")
        attach("after paint")
        let painted = try saved()
        XCTAssert(painted.contains("gain_db = 0.0"), "the stroke moved the band")

        // The band's handle, at 1 kHz and 0 dB, dragged up.
        let handle = handlePoint(graph)
        handle.press(forDuration: 0.05, thenDragTo: handle.withOffset(CGVector(dx: 0, dy: -40)))
        try waitForSave(from: painted, "dragging the handle")
        attach("after handle drag")
        let dragged = try saved()
        XCTAssertFalse(dragged.contains("gain_db = 0.0"), "the handle did not move the band")

        // A pinch on the graph widens the band last held: a lower Q.
        graph.pinch(withScale: 2, velocity: 1)
        try waitForSave(from: dragged, "the pinch")
        attach("after pinch")
        XCTAssertFalse(try saved().contains("q = 1.0"), "the pinch did not change Q")
    }

    private func saved() throws -> String {
        try String(contentsOf: config.appending(path: "config.toml"), encoding: .utf8)
    }

    private func waitForSave(from old: String, _ what: String) throws {
        for _ in 0..<20 {
            if try saved() != old { return }
            usleep(250_000)
        }
        XCTFail("\(what) saved nothing")
    }

    private func point(_ e: XCUIElement, _ x: CGFloat, _ y: CGFloat) -> XCUICoordinate {
        e.coordinate(withNormalizedOffset: CGVector(dx: x, dy: y))
    }

    /// Where 1 kHz at 0 dB is drawn: the plot is the chart less its axis
    /// labels, about 28 points on the left and 20 below, and its frequency
    /// axis runs from 20 Hz to 20 kHz on a log scale.
    private func handlePoint(_ graph: XCUIElement) -> XCUICoordinate {
        let f = graph.frame
        let plot = CGRect(x: f.minX + 28, y: f.minY, width: f.width - 28, height: f.height - 20)
        let x = plot.minX + plot.width * log(1000 / 20) / log(1000)
        return app.coordinate(withNormalizedOffset: .zero).withOffset(CGVector(dx: x, dy: plot.midY))
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
        for _ in 0..<8 where !(element.exists && element.frame.maxY < 700) {
            app.coordinate(withNormalizedOffset: CGVector(dx: 0.95, dy: 0.75))
                .press(forDuration: 0.05, thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.95, dy: 0.35)))
        }
        XCTAssert(element.waitForExistence(timeout: 5), "\(element) never appeared")
    }
}
