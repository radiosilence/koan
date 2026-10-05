import XCTest

/// A shelf's See all opens the browser filtered to the shelf: the count the
/// shelf gave is the browser's, and the preview's tracks are the head of its
/// listing, in the same order.
///
/// Runs against whatever library the simulator holds, so sign it in to a
/// server whose shelves have more than their previews show. A section that
/// shows all it has offers no See all, and is skipped.
@MainActor
final class SeeAllTests: XCTestCase {
    private var app: XCUIApplication!

    override func setUp() async throws {
        continueAfterFailure = false
        app = XCUIApplication()
        app.launch()
    }

    func testFavourites() throws { try shelf("Favourites") }

    func testRecentlyPlayed() throws { try shelf("Recently Played") }

    private func shelf(_ name: String) throws {
        var checked = 0
        for list in ["tracks", "albums", "artists"] {
            open(name)
            let button = app.buttons["see-all-\(list)"]
            guard reveal(button) else { continue }
            let total = try XCTUnwrap(Int(button.label.filter(\.isNumber)), "no count in \(button.label)")
            let preview = list == "tracks" ? trackIds() : []
            snap("\(name)-\(list)-shelf")
            button.tap()

            let noun = list.dropLast()
            let count = "\(total) \(total == 1 ? String(noun) : list)"
            XCTAssert(
                app.staticTexts[count].waitForExistence(timeout: 10),
                "\(name): See all (\(total)) opened a browser without “\(count)”"
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
        if checked == 0 { throw XCTSkip("\(name) shows everything it has") }
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
