import XCTest

/// Every scrolling page scrolls far enough that its last element clears the
/// theme's mini player and tab bar, which are drawn over the pages rather
/// than beside them, and the select mode's bar where one is up. Each page is
/// opened, scrolled to its foot, and its lowest element compared with the
/// top of what covers it.
///
/// `just ios-bars` runs this against a library of its own, served from a
/// throwaway koan: an artist with fifteen records, one of them thirty tracks
/// long, forty more artists, every record and the long one's tracks
/// favourited, and twenty playlists. Pointed elsewhere with `KOAN_REMOTE__*`,
/// it needs the same names.
@MainActor
final class BarInsetTests: XCTestCase {
    private var app: XCUIApplication!

    /// One configuration for the run, holding three EQs: the library syncs
    /// into it once, and a test after the first finds it there.
    private static let config: URL = {
        let dir = FileManager.default.temporaryDirectory.appending(path: "koan-bar-inset-\(UUID())")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try? """
        [[dsp.profiles]]
        name = "Test Tuning"
        role = "tuning"
        filters = [{ type = "low_shelf", freq = 105.0, gain_db = 3.0, q = 0.7 }]

        [[dsp.profiles]]
        name = "Test Correction"
        role = "correction"
        filters = [{ type = "peaking", freq = 3000.0, gain_db = -2.0, q = 1.0 }]
        target = { made_for = "harman-over-ear-2018" }

        [[dsp.profiles]]
        name = "Plain Correction"
        role = "correction"
        filters = [{ type = "peaking", freq = 6000.0, gain_db = -2.0, q = 1.0 }]
        """.write(to: dir.appending(path: "config.toml"), atomically: true, encoding: .utf8)
        return dir
    }()

    override func setUp() async throws {
        continueAfterFailure = true
        addUIInterruptionMonitor(withDescription: "Notifications") { alert in
            alert.buttons.element(boundBy: 0).tap()
            return true
        }
        app = XCUIApplication()
        // The account, passed through from `just ios-bars`: the test runner
        // sees `TEST_RUNNER_KOAN_*` as `KOAN_*`.
        for (key, value) in ProcessInfo.processInfo.environment where key.hasPrefix("KOAN_") {
            app.launchEnvironment[key] = value
        }
        app.launchEnvironment["KOAN_CONFIG_DIR"] = Self.config.path
        // Silent, and blind to UPnP renderers: tests share a machine with its owner.
        app.launchEnvironment["KOAN_PLAYBACK__MUTED"] = "true"
        app.launchEnvironment["KOAN_PLAYBACK__RENDERERS"] = "false"
        // The theme's bar is what is measured, whatever look the simulator
        // was left in.
        app.launchEnvironment["KOAN_APPEARANCE__THEME"] = "koan"
        // The wash held still, so the app goes idle between steps.
        app.launchArguments += ["-graphics", "1"]
        app.launch()
        // The first launch syncs the library; the shelves fill once it has.
        XCTAssertTrue(app.otherElements["koan-bar"].waitForExistence(timeout: 20), "no koan-bar: is the kōan look on?")
        settle(4)
    }

    func testQueue() {
        playLongAlbum()
        tab("Queue")
        assertClears("queue")
        selecting { assertClears("queue, selecting") }
    }

    func testLibrary() {
        // Plays first, so history, recently played and the downloads have rows.
        playLongAlbum()
        skip(4)
        let pad = UIDevice.current.userInterfaceIdiom == .pad
        if !pad {
            tab("Library")
            assertClears("library")
        }
        for (row, name) in [
            ("Albums", "albums"), ("Artists", "artists"), ("Favourites", "favourites"),
            ("Recently Played", "recently played"), ("Downloaded", "downloaded"),
            ("History", "history"), ("Downloads", "downloads"),
        ] {
            guard open(page(row)) else {
                XCTFail("no \(row) in the library")
                continue
            }
            settle(2)
            assertClears(name)
            if app.buttons[any: "Select"].exists {
                selecting { assertClears("\(name), selecting") }
            }
            if !pad { back() }
        }
        if open(page("Playlists")) {
            settle(2)
            assertClears("playlists")
            // The last one, which the scroll to the foot left on screen.
            let last = app.descendants(matching: .any)
                .matching(NSPredicate(format: "label BEGINSWITH[c] 'Playlist 20'")).firstMatch
            if open(last) {
                settle(2)
                assertClears("playlist")
                selecting { assertClears("playlist, selecting") }
            } else {
                XCTFail("no Playlist 20")
            }
        }
    }

    func testAlbumAndArtist() {
        search("Thirty Rooms")
        XCTAssertTrue(open(app.staticTexts[any: "Thirty Rooms"]), "no Thirty Rooms in the results")
        settle(2)
        assertClears("album")
        selecting { assertClears("album, selecting") }

        search("Long Artist")
        XCTAssertTrue(open(app.buttons[any: "Long Artist"]), "no Long Artist in the results")
        settle(2)
        assertClears("artist")
        selecting { assertClears("artist, selecting") }
    }

    func testSearchResults() {
        search("Song")
        assertClears("search results")
        selecting { assertClears("search results, selecting") }
    }

    func testHistory() {
        // A few plays, so the page has rows.
        playLongAlbum()
        skip(4)
        let pad = UIDevice.current.userInterfaceIdiom == .pad
        if !pad { tab("Library") }
        if open(page("History")) {
            settle(2)
            assertClears("history")
            selecting { assertClears("history, selecting") }
        }
    }

    func testSettings() {
        tab("Settings")
        assertClears("settings")
        for pane in ["Server", "Account", "People", "Playback", "Devices", "Integrations", "Appearance"] {
            guard open(app.buttons[any: pane]) else { continue }
            settle(1)
            assertClears("settings: \(pane)")
            back()
        }
    }

    func testEq() {
        tab("Settings")
        XCTAssertTrue(open(app.buttons[any: "EQ"]), "no EQ in Settings")
        settle(1)
        assertClears("EQ")
        let manage = app.buttons[any: "Manage EQ"]
        XCTAssertTrue(open(manage), "no Manage EQ")
        settle(1)
        assertClears("Manage EQ")

        // The pushed lists of a profile's pickers.
        for (profile, picker) in [
            ("Test Tuning", "Made against"),
            ("Test Correction", "Corrected to"),
            ("Plain Correction", "Made for"),
        ] {
            let row = app.buttons.matching(NSPredicate(format: "label CONTAINS[c] %@", profile)).firstMatch
            reach(row)
            guard open(row) else {
                XCTFail("no \(profile) in Manage EQ")
                continue
            }
            settle(1)
            assertClears("EQ: \(profile)")
            // Near the top, over the foot the page was scrolled to.
            for _ in 0..<6 { app.swipeDown(velocity: .fast) }
            let link = app.buttons.matching(NSPredicate(format: "label BEGINSWITH[c] %@", picker)).firstMatch
            reach(link)
            if open(link) {
                settle(1)
                assertClears("EQ: \(picker)")
                back()
            } else {
                XCTFail("no \(picker) on \(profile)")
            }
            back()
        }
    }

    // MARK: - The check

    /// Scrolls the page to its foot and checks its lowest element ends at or
    /// above the top of the koan-bar, or of the select mode's bar while one is up.
    private func assertClears(_ page: String, file: StaticString = #filePath, line: UInt = #line) {
        scrollToFoot()
        snap(page)
        let bar = app.otherElements["koan-bar"]
        guard bar.exists else {
            XCTFail("\(page): no koan-bar", file: file, line: line)
            return
        }
        var top = bar.frame.minY
        let picking = app.otherElements[any: "Selection"]
        if picking.exists { top = min(top, picking.frame.minY) }
        // A page showing only its empty state has no list to end under the bar.
        guard let foot = foot() else { return }
        XCTAssertLessThanOrEqual(
            foot.maxY, top + 1,
            "\(page): “\(foot.label)” ends at \(foot.maxY), under the bar from \(top)",
            file: file, line: line
        )
    }

    /// The lowest element of the page's scrolling content: the leaves of the
    /// widest scroll view on screen, which is the page's own, not the bars'.
    private func foot() -> (maxY: CGFloat, label: String)? {
        guard let root = try? app.snapshot() else { return nil }
        let screen = root.frame
        var scrolls: [XCUIElementSnapshot] = []
        func gather(_ s: XCUIElementSnapshot) {
            if s.identifier == "koan-bar" || s.label == "Selection" { return }
            if [.collectionView, .scrollView, .table].contains(s.elementType),
               s.frame.minX >= screen.minX - 1, s.frame.maxX <= screen.maxX + 1, s.frame.height > 0 {
                scrolls.append(s)
            }
            s.children.forEach(gather)
        }
        gather(root)
        guard let page = scrolls.max(by: { $0.frame.width * $0.frame.height < $1.frame.width * $1.frame.height }) else { return nil }
        let content: Set<XCUIElement.ElementType> = [
            .staticText, .button, .image, .switch, .slider, .textField, .link, .cell, .toggle, .segmentedControl,
        ]
        var lowest: (maxY: CGFloat, label: String)?
        func walk(_ s: XCUIElementSnapshot) {
            if s.children.isEmpty || s.elementType == .button || s.elementType == .switch {
                if content.contains(s.elementType), s.frame.height > 0,
                   s.frame.minX < page.frame.maxX, s.frame.maxX > page.frame.minX,
                   s.frame.minY < screen.maxY, s.frame.maxY > lowest?.maxY ?? -.infinity {
                    lowest = (s.frame.maxY, s.label.isEmpty ? "\(s.elementType)" : s.label)
                }
                if s.children.isEmpty { return }
            }
            s.children.forEach(walk)
        }
        page.children.forEach(walk)
        return lowest
    }

    /// Drags up from the right edge, which no chart or slider takes, until the
    /// page's foot stops moving.
    private func scrollToFoot() {
        var last = foot()?.maxY
        var still = 0
        for _ in 0..<30 where still < 2 {
            app.coordinate(withNormalizedOffset: CGVector(dx: 0.96, dy: 0.7))
                .press(forDuration: 0.05, thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.96, dy: 0.2)),
                       withVelocity: .fast, thenHoldForDuration: 0)
            let now = foot()?.maxY
            still = now == last ? still + 1 : 0
            last = now
        }
        settle(1)
    }

    // MARK: - Getting about

    /// Plays the long record from its first track, which queues the rest.
    private func playLongAlbum() {
        search("Thirty Rooms")
        XCTAssertTrue(open(app.staticTexts[any: "Thirty Rooms"]), "no Thirty Rooms in the results")
        settle(2)
        let first = app.cells.containing(NSPredicate(format: "label ==[c] 'Room 1'")).firstMatch
        XCTAssertTrue(open(first), "no Room 1 on the record")
        XCTAssertTrue(app.buttons[any: "Pause"].firstMatch.waitForExistence(timeout: 30), "the record did not play")
    }

    /// On through the queue, so each track is a play in history, then paused:
    /// plays still arriving push a list's foot down while it is measured, and
    /// an app playing is never idle for the test driver to wait on.
    private func skip(_ tracks: Int) {
        for _ in 0..<tracks {
            app.buttons[any: "Next"].firstMatch.tap()
            settle(2)
        }
        let pause = app.buttons[any: "Pause"].firstMatch
        if pause.exists { pause.tap() }
        settle(1)
    }

    private func search(_ text: String) {
        tab("Search")
        var field = app.searchFields.firstMatch
        if !field.waitForExistence(timeout: 5) {
            field = app.textFields[any: "Artists, albums, tracks"]
        }
        XCTAssertTrue(field.waitForExistence(timeout: 5), "no search field")
        field.tap()
        if let old = field.value as? String, !old.isEmpty, old != field.placeholderValue {
            field.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: old.count))
        }
        field.typeText(text + "\n")
        settle(3)
    }

    /// In select mode, with two of the page ticked where it has two, then out.
    private func selecting(_ body: () -> Void) {
        let select = app.buttons[any: "Select"]
        guard select.waitForExistence(timeout: 3) else {
            XCTFail("no Select")
            return
        }
        select.tap()
        settle(1)
        body()
        let done = app.buttons[any: "Done"]
        if done.exists { done.tap() }
        settle(1)
    }

    private func tab(_ name: String) {
        let button = app.buttons[any: name]
        if button.waitForExistence(timeout: 3) {
            button.tap()
        } else {
            // An iPad's sidebar: rows whose label is their text.
            app.staticTexts[any: name].tap()
        }
        settle(1)
    }

    /// A library page: a row of the phone's Library tab, or of an iPad's sidebar.
    private func page(_ name: String) -> XCUIElement {
        let button = app.buttons[any: name]
        return button.exists ? button : app.staticTexts[any: name]
    }

    private func open(_ element: XCUIElement) -> Bool {
        guard element.waitForExistence(timeout: 5) else { return false }
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
        settle(1)
    }

    /// Scrolls until `element` is clear of the bars.
    private func reach(_ element: XCUIElement) {
        let bar = app.otherElements["koan-bar"]
        for _ in 0..<8 where !(element.exists && element.frame.maxY < (bar.exists ? bar.frame.minY : 700)) {
            app.coordinate(withNormalizedOffset: CGVector(dx: 0.96, dy: 0.7))
                .press(forDuration: 0.05, thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.96, dy: 0.4)))
        }
    }

    private func settle(_ seconds: TimeInterval) {
        _ = XCTWaiter.wait(for: [expectation(description: "settle")], timeout: seconds)
    }

    private func snap(_ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }
}
