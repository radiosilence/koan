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
        // The theme's tab bar is buttons of its own, not a system tab bar.
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
        XCTAssertEqual(band("gain_db", in: painted), 0, "the stroke moved the band")

        // The band's handle, at 1 kHz and 0 dB, dragged up.
        let handle = handlePoint(graph)
        handle.press(forDuration: 0.05, thenDragTo: handle.withOffset(CGVector(dx: 0, dy: -40)))
        try waitForSave(from: painted, "dragging the handle")
        attach("after handle drag")
        let dragged = try saved()
        XCTAssertGreaterThan(band("gain_db", in: dragged) ?? 0, 0.5, "the handle did not raise the band")

        // A pinch on the graph widens the band last held: a lower Q.
        graph.pinch(withScale: 2, velocity: 1)
        try waitForSave(from: dragged, "the pinch")
        attach("after pinch")
        XCTAssertLessThan(band("q", in: try saved()) ?? 1, 0.9, "the pinch did not lower Q")
    }

    /// A figure of the peaking band as saved, or nil where it is left out.
    private func band(_ key: String, in config: String) -> Double? {
        guard let start = config.range(of: "\"peaking\"") else { return nil }
        let rest = config[start.upperBound...]
        let end = rest.range(of: "type")?.lowerBound ?? rest.endIndex
        let pattern = "\\b\(key)\\s*=\\s*(-?[0-9.]+)"
        guard let match = rest[..<end].firstMatch(of: try! Regex(pattern)),
              let value = match.output[1].substring else { return key == "gain_db" ? 0 : nil }
        return Double(value)
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
        if let text = try? saved() {
            let saved = XCTAttachment(string: text)
            saved.name = "\(name) config"
            saved.lifetime = .keepAlways
            add(saved)
        }
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
