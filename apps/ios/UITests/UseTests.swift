import XCTest

/// Uses the app the way someone listening does, and checks each thing worked:
/// browse, play, pause, skip, queue, search, favourite, playlists, and playing
/// on with the app in the background.
///
/// Needs a simulator signed in to a server (`just ios-signin`). Screenshots of
/// each step are attached to the result bundle.
@MainActor
final class UseTests: XCTestCase {
    private var app: XCUIApplication!

    override func setUp() async throws {
        continueAfterFailure = true
        app = XCUIApplication()
        app.launch()
    }

    func testListening() throws {
        pause(3)

        // Browse to a record and play its first track.
        tab("Library")
        tap(app.buttons["Albums"].firstMatch, "Albums")
        pause(3)
        snap("use-01-albums")
        app.coordinate(withNormalizedOffset: CGVector(dx: 0.27, dy: 0.3)).tap()
        pause(3)
        snap("use-02-album")
        let firstTrack = app.cells.element(boundBy: 0)
        XCTAssert(firstTrack.waitForExistence(timeout: 5), "album has no tracks")
        firstTrack.tap()
        // A first play downloads the track; allow for a slow network.
        XCTAssert(app.buttons["Pause"].firstMatch.waitForExistence(timeout: 30), "tapping a track did not start playback")
        pause(5)
        snap("use-03-playing")

        // Pause, resume, skip.
        tap(app.buttons["Pause"].firstMatch, "Pause")
        XCTAssert(app.buttons["Play"].firstMatch.waitForExistence(timeout: 5), "pause did not pause")
        tap(app.buttons["Play"].firstMatch, "Play")
        XCTAssert(app.buttons["Pause"].firstMatch.waitForExistence(timeout: 5), "play did not resume")
        tap(app.buttons["Next"].firstMatch, "Next")
        pause(4)
        snap("use-04-skipped")

        // Favourite the first track on the page.
        let heart = app.buttons["Favourite"].firstMatch
        if heart.waitForExistence(timeout: 3) {
            heart.tap()
            pause(1)
            snap("use-05-favourited")
        } else {
            XCTFail("no favourite button on the album page")
        }

        // Queue another record from the grid's context menu.
        back()
        pause(2)
        app.coordinate(withNormalizedOffset: CGVector(dx: 0.73, dy: 0.3)).press(forDuration: 1.2)
        let addToQueue = app.buttons["Add to Queue"].firstMatch
        if addToQueue.waitForExistence(timeout: 4) {
            addToQueue.tap()
        } else {
            XCTFail("no Add to Queue in the album's menu")
            app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.05)).tap()
        }
        pause(2)
        tab("Queue")
        pause(2)
        snap("use-06-queue")

        // Now Playing, and its lyrics toggle. The mini player sits above the
        // tab bar on a phone and at the foot of the screen on an iPad.
        let pad = UIDevice.current.userInterfaceIdiom == .pad
        app.coordinate(withNormalizedOffset: CGVector(dx: pad ? 0.2 : 0.35, dy: pad ? 0.965 : 0.868)).tap()
        pause(3)
        snap("use-07-now-playing")
        let lyrics = app.buttons["Show lyrics"].firstMatch
        if lyrics.waitForExistence(timeout: 3) {
            lyrics.tap()
            pause(2)
            snap("use-08-lyrics")
            // Tapping a line seeks, and once crashed the app (#215). Only when
            // there are lines: most tracks on a demo server have none.
            if !app.staticTexts["No lyrics found"].exists {
                app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.45)).tap()
                pause(2)
            }
            XCTAssertEqual(app.state, .runningForeground, "tapping a lyric line crashed the app")
            let artwork = app.buttons["Show artwork"].firstMatch
            if artwork.exists { artwork.tap() }
        }
        app.swipeDown(velocity: .fast)
        pause(2)

        // Favourites and playlists.
        tab("Library")
        back()
        if tap(app.buttons["Favourites"].firstMatch, "Favourites") {
            pause(3)
            snap("use-09-favourites")
            back()
        }
        if tap(app.buttons["Playlists"].firstMatch, "Playlists") {
            pause(3)
            snap("use-10-playlists")
            app.cells.element(boundBy: 0).tap()
            pause(3)
            snap("use-11-playlist")
            back()
            back()
        }

        // Search, and play from the results.
        tab("Search")
        let field = app.searchFields.firstMatch
        if field.waitForExistence(timeout: 8) {
            field.tap()
            field.typeText("Brock")
            pause(3)
            snap("use-12-search")
        } else {
            XCTFail("no search field")
        }

        // Keeps playing with the app in the background.
        XCUIDevice.shared.press(.home)
        pause(6)
        app.activate()
        pause(2)
        XCTAssert(app.buttons["Pause"].firstMatch.waitForExistence(timeout: 5), "playback stopped in the background")
        snap("use-13-back-from-background")
    }

    private func tab(_ name: String) {
        let bar = app.tabBars.buttons[name]
        if bar.waitForExistence(timeout: 3) { bar.tap() } else { app.buttons[name].firstMatch.tap() }
        pause(1)
    }

    @discardableResult
    private func tap(_ element: XCUIElement, _ what: String) -> Bool {
        guard element.waitForExistence(timeout: 4) else {
            XCTFail("no \(what)")
            return false
        }
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
