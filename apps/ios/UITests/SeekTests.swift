import XCTest

/// Seeks far into a track that is still downloading, the moment it starts.
///
/// A seek past what has arrived once took the process down (#215), and a
/// seek bar dragged straight after pressing play is the ordinary way to reach
/// it. Needs a signed-in simulator with albums not yet cached.
@MainActor
final class SeekTests: XCTestCase {
    func testSeekWhileDownloading() throws {
        let app = XCUIApplication()
        app.launch()
        pause(3)

        let library = app.tabBars.buttons["Library"]
        if library.waitForExistence(timeout: 3) { library.tap() } else { app.buttons["Library"].firstMatch.tap() }
        app.buttons["Albums"].firstMatch.tap()
        pause(3)
        // Further down the grid than the walks go, so likely not cached.
        app.swipeUp()
        pause(2)
        app.coordinate(withNormalizedOffset: CGVector(dx: 0.73, dy: 0.45)).tap()
        pause(3)
        app.cells.element(boundBy: 0).tap()
        pause(1)

        // Straight into Now Playing and to near the end of the track.
        app.coordinate(withNormalizedOffset: CGVector(dx: 0.35, dy: 0.868)).tap()
        pause(2)
        let bar = app.sliders.firstMatch
        if bar.waitForExistence(timeout: 3) {
            bar.adjust(toNormalizedSliderPosition: 0.9)
        } else {
            // The seek bar is drawn, not a slider: drag across it.
            let from = app.coordinate(withNormalizedOffset: CGVector(dx: 0.2, dy: 0.907))
            from.press(forDuration: 0.1, thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.8, dy: 0.907)))
        }
        pause(1)
        attach("seek-01-after-seek")
        pause(8)
        XCTAssertEqual(app.state, .runningForeground, "seeking while downloading crashed the app")
        attach("seek-02-later")

        // And a second seek back to the start while it may still be arriving.
        let from = app.coordinate(withNormalizedOffset: CGVector(dx: 0.8, dy: 0.907))
        from.press(forDuration: 0.1, thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.1, dy: 0.907)))
        pause(6)
        XCTAssertEqual(app.state, .runningForeground, "seeking back crashed the app")
        attach("seek-03-back")
    }

    private func pause(_ seconds: TimeInterval) {
        _ = XCTWaiter.wait(for: [expectation(description: "settle")], timeout: seconds)
    }

    private func attach(_ name: String) {
        let shot = XCTAttachment(screenshot: XCUIApplication().screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }
}
