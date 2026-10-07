import XCTest

/// The route that once left an album page empty (#997): search for an artist,
/// play them, open Now Playing and close it again, then follow its album link.
/// Every album page reached has its tracks, never the placeholder over none.
///
/// Needs a library holding the artist `KOAN_ALBUM_ARTIST` (default "Halden",
/// in the simulator's fixture library). `just ios-album-page` runs it.
@MainActor
final class AlbumPageTests: XCTestCase {
    private var app: XCUIApplication!
    private let env = ProcessInfo.processInfo.environment

    override func setUp() async throws {
        continueAfterFailure = false
        app = XCUIApplication()
        // Silent, and blind to UPnP renderers: tests share a machine with its owner.
        app.launchEnvironment["KOAN_PLAYBACK__MUTED"] = "true"
        app.launchEnvironment["KOAN_PLAYBACK__RENDERERS"] = "false"
        app.launch()
    }

    func testNowPlayingRoundTripLeavesNoEmptyAlbum() throws {
        let artist = env["KOAN_ALBUM_ARTIST"] ?? "Halden"
        pause(3)

        tab("Search")
        var field = app.searchFields.firstMatch
        if !field.waitForExistence(timeout: 8) {
            field = app.textFields[any: "Artists, albums, tracks"]
        }
        XCTAssertTrue(field.waitForExistence(timeout: 5), "search field")
        field.tap()
        field.typeText(artist.lowercased())
        pause(3)
        // Off the keyboard, so the results and the tab bar are reachable.
        app.swipeDown(velocity: .slow)

        let name = onScreen(app.buttons.matching(NSPredicate(format: "label ==[c] %@", artist)))
        XCTAssertTrue(name.waitForExistence(timeout: 5), "artist \(artist) in the results")
        name.tap()
        pause(3)
        snap("01-artist")

        let play = onScreen(app.buttons.matching(NSPredicate(format: "label ==[c] %@", "Play")))
        XCTAssertTrue(play.waitForExistence(timeout: 5), "the artist's Play")
        play.tap()
        _ = app.buttons[any: "Pause"].firstMatch.waitForExistence(timeout: 30)
        pause(2)

        openNowPlaying()
        snap("02-now-playing")
        app.swipeDown(velocity: .fast)
        pause(2)
        snap("03-after-now-playing")
        assertNoEmptyAlbum("after closing Now Playing")

        // The album link under the artist's name: the one way from Now Playing
        // to a record.
        openNowPlaying()
        let link = try XCTUnwrap(albumLink(under: artist), "Now Playing's album link")
        let album = link.label
        link.tap()
        pause(3)
        snap("04-album-from-now-playing")
        assertNoEmptyAlbum("on the playing record")
        let title = app.staticTexts.matching(NSPredicate(format: "label ==[c] %@", album)).firstMatch
        XCTAssertTrue(title.waitForExistence(timeout: 5), "the playing record's page, titled \(album)")
        XCTAssertFalse(app.staticTexts[any: "Album"].exists, "the placeholder title")
    }

    /// The page that #997 was: a title of "Album" over "No tracks", or the
    /// record named as gone.
    private func assertNoEmptyAlbum(_ when: String, file: StaticString = #filePath, line: UInt = #line) {
        let empty = app.staticTexts[any: "No tracks"]
        let gone = app.staticTexts.matching(NSPredicate(format: "label CONTAINS[c] %@", "isn't in the library")).firstMatch
        let failed = app.staticTexts[any: "Couldn't read this album"]
        XCTAssertFalse(empty.exists && empty.isHittable, "an album page with no tracks \(when)", file: file, line: line)
        XCTAssertFalse(gone.exists, "an album page for a record not in the library \(when)", file: file, line: line)
        XCTAssertFalse(failed.exists, "an album page whose read failed \(when)", file: file, line: line)
    }

    /// The album link in Now Playing: the line straight under the artist's
    /// name, which the sheet draws only for a track with an album.
    private func albumLink(under artist: String) -> XCUIElement? {
        _ = app.buttons[any: "Show lyrics"].waitForExistence(timeout: 5)
        let texts = app.staticTexts.allElementsBoundByIndex.filter { $0.isHittable }
        // The sheet's, below the mini player's copy behind it.
        guard let name = texts.filter({ $0.label.caseInsensitiveCompare(artist) == .orderedSame })
            .max(by: { $0.frame.minY < $1.frame.minY })
        else { return nil }
        return texts.filter { $0.frame.minY >= name.frame.maxY && $0.frame.minY < name.frame.maxY + 40 }
            .min { $0.frame.minY < $1.frame.minY }
    }

    private func tab(_ name: String) {
        let button = app.tabBars.buttons[any: name]
        if button.waitForExistence(timeout: 3) {
            button.tap()
        } else {
            app.buttons[any: name].tap()
        }
        pause(1)
    }

    /// The mini player: above the tab bar on a phone, at the foot on an iPad.
    private func openNowPlaying() {
        let pad = UIDevice.current.userInterfaceIdiom == .pad
        for _ in 0..<3 where !app.buttons[any: "Show lyrics"].exists {
            app.coordinate(withNormalizedOffset: CGVector(dx: pad ? 0.2 : 0.35, dy: pad ? 0.965 : 0.86)).tap()
            _ = app.buttons[any: "Show lyrics"].waitForExistence(timeout: 5)
        }
        pause(2)
    }

    private func onScreen(_ query: XCUIElementQuery) -> XCUIElement {
        _ = query.firstMatch.waitForExistence(timeout: 3)
        return query.allElementsBoundByIndex.last { $0.isHittable } ?? query.firstMatch
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
