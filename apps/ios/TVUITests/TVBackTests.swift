import XCTest

/// The remote's Menu button, as tvOS has it: back a page, then from a tab's
/// root up to the tab bar, and only from the tab bar out of the app. And the
/// controls that were once out of the remote's reach. `just tv-back` runs it
/// against a throwaway koan serving a generated library.
@MainActor
final class TVBackTests: XCTestCase {
    private var app: XCUIApplication!
    private let remote = XCUIRemote.shared

    override func setUp() async throws {
        continueAfterFailure = false
        app = XCUIApplication()
        app.launchEnvironment["KOAN_PLAYBACK__MUTED"] = "true"
        app.launchEnvironment["KOAN_PLAYBACK__RENDERERS"] = "false"
        app.launchEnvironment["KOAN_DEVICES__NEARBY"] = "false"
        for (key, value) in ProcessInfo.processInfo.environment where key.hasPrefix("KOAN_") {
            app.launchEnvironment[key] = value
        }
        app.launch()
        pause(5)
    }

    /// The tabs, left to right.
    private enum Tab: Int {
        case nowPlaying, queue, library, search, settings
    }

    /// Two pages deep in the library, Menu twice comes back to the library's
    /// index, a third time reaches the tab bar, and a fourth leaves.
    func testMenuPopsThenFocusesTabBarThenLeaves() {
        open(.library)
        let index = app.buttons[any: "History"]
        reach(app.buttons[any: "Albums"], by: .down)
        press(.select)
        pause(3)
        XCTAssertFalse(index.exists, "Albums is pushed over the index")
        press(.down)
        press(.select)
        pause(3)
        snap("1-album")

        press(.menu)
        pause(2)
        XCTAssertEqual(app.state, .runningForeground, "Menu on a record goes back, not out")
        XCTAssertFalse(index.exists, "one page back is the album grid")

        press(.menu)
        pause(2)
        XCTAssertEqual(app.state, .runningForeground, "Menu on the grid goes back, not out")
        XCTAssertTrue(index.waitForExistence(timeout: 3), "two pages back is the library's index")

        press(.menu)
        pause(2)
        XCTAssertEqual(app.state, .runningForeground, "Menu at a tab's root goes to the tab bar")
        XCTAssertTrue(app.buttons[any: "Library"].hasFocus, "the tab bar has focus, on the tab it came from")
        snap("2-tab-bar")

        press(.menu)
        XCTAssertTrue(
            app.wait(for: .runningBackground, timeout: 5) || app.state == .runningBackgroundSuspended,
            "Menu on the tab bar leaves the app"
        )
    }

    /// Playing from an artist page shows the queue over it, in the same tab:
    /// Menu goes back to the artist. It once moved to the Queue tab with focus
    /// on the tab bar, where Menu left the app.
    func testMenuAfterPlayingGoesBackToTheArtist() {
        open(.library)
        reach(app.buttons[any: "Artists"], by: .down)
        press(.select)
        pause(3)
        press(.down)
        press(.select)
        pause(3)
        let play = app.buttons[any: "Play"]
        reach(play, by: .down)
        press(.select)
        pause(4)
        snap("3-played")
        XCTAssertFalse(app.buttons[any: "Queue"].hasFocus || app.buttons[any: "Library"].hasFocus, "focus stays on the page")

        press(.menu)
        pause(2)
        XCTAssertEqual(app.state, .runningForeground, "Menu on the queue goes back, not out")
        XCTAssertTrue(play.waitForExistence(timeout: 3), "back on the artist's page")
        snap("4-back-on-artist")
    }

    /// A settings pane, pushed by link, goes back too.
    func testMenuLeavesSettingsPane() {
        open(.settings)
        reach(app.buttons[any: "Server"], by: .down)
        press(.select)
        pause(3)
        XCTAssertFalse(app.buttons[any: "Playback"].exists, "the Server pane is pushed over the list")
        snap("5-settings-pane")
        press(.menu)
        pause(2)
        XCTAssertEqual(app.state, .runningForeground, "Menu on a settings pane goes back, not out")
        XCTAssertTrue(app.buttons[any: "Playback"].waitForExistence(timeout: 3), "back on the settings list")
        press(.menu)
        pause(2)
        XCTAssertEqual(app.state, .runningForeground, "Menu at Settings goes to the tab bar")
        XCTAssertTrue(app.buttons[any: "Settings"].hasFocus, "the tab bar has focus")
    }

    /// EQ's empty stages take focus and open the account's own profiles.
    func testEqPlaceholdersFocusAndOpen() {
        open(.settings)
        reach(app.buttons[any: "EQ"], by: .down)
        press(.select)
        pause(3)
        let add = app.buttons.containing(NSPredicate(format: "label CONTAINS[c] %@", "add a correction")).firstMatch
        XCTAssertTrue(add.waitForExistence(timeout: 5), "the correction's placeholder is a button")
        reach(add, by: .down)
        snap("6-eq-placeholder")
        press(.select)
        pause(2)
        snap("7-eq-picker")
        let empty = app.staticTexts.containing(NSPredicate(format: "label CONTAINS[c] %@", "phone or Mac")).firstMatch
        XCTAssertTrue(empty.waitForExistence(timeout: 3), "with nothing to choose, the picker says where to add one")
        press(.menu)
        pause(2)
        XCTAssertEqual(app.state, .runningForeground, "Menu closes the picker")
    }

    /// Search's artist pills take focus and open the artist.
    func testSearchArtistPillsFocus() {
        open(.search)
        press(.down)
        let field = app.searchFields.firstMatch
        XCTAssertTrue(field.waitForExistence(timeout: 10), "the search field")
        field.typeText("Artist 1")
        pause(4)
        XCTAssertTrue(app.buttons[any: "Artist 1"].waitForExistence(timeout: 5), "the artists' pills are buttons")
        // Down from the keyboard lands on the section's heading, then on
        // whichever pill is under it.
        // Any type: the keyboard's keys are not buttons.
        let focused = app.descendants(matching: .any).matching(NSPredicate(format: "hasFocus == true")).firstMatch
        for _ in 0..<3 where !focused.label.hasPrefix("Artist ") {
            press(.down)
        }
        snap("8-pill-focused")
        XCTAssertTrue(focused.label.hasPrefix("Artist "), "an artist's pill has focus, not \(focused.label)")
        let name = focused.label
        press(.select)
        pause(3)
        XCTAssertTrue(app.buttons[any: "Play"].waitForExistence(timeout: 5), "the artist's page, with its Play")
        XCTAssertTrue(app.staticTexts[any: name].exists, "the page is \(name)'s")
        snap("9-artist")
    }

    /// The album browser narrows by name from a field above it, drawn as the
    /// theme's square box.
    func testAlbumsFilterByName() {
        open(.library)
        reach(app.buttons[any: "Albums"], by: .down)
        press(.select)
        pause(3)
        XCTAssertTrue(record("Album 1").waitForExistence(timeout: 5), "the grid, unfiltered")
        let field = app.textFields["name-filter"]
        reach(field, by: .up)
        snap("10-filter-focused")
        press(.select)
        pause(2)
        app.typeText("Album 3")
        pause(1)
        press(.menu)
        pause(3)
        XCTAssertEqual(app.state, .runningForeground, "Menu leaves the keyboard, not the app")
        snap("11-filtered")
        XCTAssertTrue(record("Album 3").exists, "the record named stays")
        XCTAssertFalse(record("Album 1").exists, "records not named go")
    }

    /// A record's tile by its title, however its label runs on: "Album 1"
    /// and not "Album 10".
    private func record(_ title: String) -> XCUIElement {
        app.descendants(matching: .any)
            .matching(NSPredicate(format: "label MATCHES[c] %@", "\(title)(,.*)?"))
            .firstMatch
    }

    /// Along the tab bar to `tab`.
    private func open(_ tab: Tab) {
        press(.right, times: tab.rawValue)
        pause(2)
    }

    /// Press `direction` until `element` has focus. Counting presses breaks
    /// whenever a control comes or goes, or a press lands before focus has.
    private func reach(_ element: XCUIElement, by direction: XCUIRemote.Button) {
        XCTAssertTrue(element.waitForExistence(timeout: 5), "\(element) exists")
        for _ in 0..<12 where !element.hasFocus {
            press(direction)
        }
        if !element.hasFocus { snap("unreached") }
        XCTAssertTrue(element.hasFocus, "\(element) has focus")
    }

    private func press(_ button: XCUIRemote.Button, times: Int = 1) {
        for _ in 0..<times {
            remote.press(button)
            pause(0.6)
        }
    }

    private func pause(_ seconds: Double) {
        Thread.sleep(forTimeInterval: seconds)
    }

    private func snap(_ name: String) {
        let shot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }
}
