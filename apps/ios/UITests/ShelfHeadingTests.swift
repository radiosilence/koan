import XCTest

/// A shelf's section headings open the browser filtered to the shelf: the
/// count a heading gives is the browser's, and the preview's tracks are the
/// head of its listing, in the same order, whether or not the preview shows
/// them all.
///
/// Runs against whatever library the simulator holds, so sign it in to a
/// server with something on its shelves.
@MainActor
final class ShelfHeadingTests: XCTestCase {
    private var app: XCUIApplication!

    override func setUp() async throws {
        continueAfterFailure = false
        // A signed-in app asks to send notifications on launch.
        addUIInterruptionMonitor(withDescription: "Notifications") { alert in
            alert.buttons.element(boundBy: 0).tap()
            return true
        }
        app = XCUIApplication()
        app.launch()
    }

    func testFavourites() throws { try shelf("Favourites") }

    func testRecentlyPlayed() throws { try shelf("Recently Played") }

    private func shelf(_ name: String) throws {
        var checked = 0
        for list in ["tracks", "albums", "artists"] {
            open(name)
            let heading = app.buttons["heading-\(list)"]
            guard reveal(heading) else { continue }
            let total = try XCTUnwrap(
                Int(heading.label.filter(\.isNumber)), "no count in “\(heading.label)”"
            )
            let preview = list == "tracks" ? trackIds() : []
            snap("\(name)-\(list)-shelf")
            heading.tap()

            let noun = String(list.dropLast())
            let count = "\(total) \(total == 1 ? noun : list)"
            XCTAssert(
                app.staticTexts[count].waitForExistence(timeout: 10),
                "\(name): the \(list) heading said \(total) and opened a browser without “\(count)”"
            )
            if list == "tracks" {
                let listed = trackIds()
                let head = min(preview.count, listed.count)
                XCTAssertGreaterThan(head, 0, "no rows to compare")
                XCTAssertEqual(
                    Array(listed.prefix(head)), Array(preview.prefix(head)),
                    "\(name): the browser does not begin with the shelf's preview"
                )
            }
            snap("\(name)-\(list)-browser")
            checked += 1
        }
        if checked == 0 { throw XCTSkip("\(name) is empty") }
    }

    /// The shelf, from the Library tab's root.
    private func open(_ name: String) {
        let bar = app.tabBars.buttons["Library"]
        if bar.waitForExistence(timeout: 10) { bar.tap() } else { app.buttons["Library"].firstMatch.tap() }
        // Tapping the selected tab again goes back to its root.
        if !app.buttons[name].waitForExistence(timeout: 2) { bar.tap() }
        app.buttons[name].firstMatch.tap()
        _ = app.staticTexts[name].waitForExistence(timeout: 10)
    }

    /// Scroll until `element` is on screen; false when it never appears.
    private func reveal(_ element: XCUIElement) -> Bool {
        for _ in 0..<6 {
            if element.waitForExistence(timeout: 2), element.isHittable { return true }
            app.swipeUp()
        }
        return false
    }

    /// The track rows on screen, top to bottom, by the track id each carries.
    private func trackIds() -> [String] {
        let rows = app.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier BEGINSWITH 'track-'"))
            .allElementsBoundByIndex
            .filter { $0.frame.height > 0 && app.frame.intersects($0.frame) }
        var seen: [String: CGFloat] = [:]
        for row in rows where seen[row.identifier] == nil { seen[row.identifier] = row.frame.minY }
        return seen.sorted { $0.value < $1.value }.map(\.key)
    }

    private func snap(_ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }
}
