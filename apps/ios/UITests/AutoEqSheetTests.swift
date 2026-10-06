import XCTest

/// Find in AutoEQ stays open while AutoEQ's index arrives. The first open on a
/// fresh install fetches the index, so run it against a fresh install
/// (`just ios-autoeq-sheet` uninstalls the app first).
@MainActor
final class AutoEqSheetTests: XCTestCase {
    private var app: XCUIApplication!

    override func setUp() async throws {
        continueAfterFailure = false
        addUIInterruptionMonitor(withDescription: "Notifications") { alert in
            alert.buttons.element(boundBy: 0).tap()
            return true
        }
        app = XCUIApplication()
        app.launchEnvironment["KOAN_PLAYBACK__MUTED"] = "true"
        app.launchEnvironment["KOAN_PLAYBACK__RENDERERS"] = "false"
        app.launch()
    }

    func testFindInAutoEqStaysOpen() throws {
        let settings = app.tabBars.buttons["Settings"]
        if settings.waitForExistence(timeout: 10) {
            settings.tap()
        } else {
            app.buttons["Settings"].firstMatch.tap()
        }
        let eq = app.buttons["EQ"]
        XCTAssert(eq.waitForExistence(timeout: 10), "no EQ in Settings")
        eq.tap()
        let find = app.buttons["Find in AutoEQ…"]
        for _ in 0..<6 where !(find.exists && find.isHittable) {
            app.swipeUp()
        }
        XCTAssert(find.waitForExistence(timeout: 5), "no Find in AutoEQ on the EQ page")
        find.tap()

        let cancel = app.buttons["Cancel"]
        XCTAssert(cancel.waitForExistence(timeout: 5), "the sheet did not open")
        // The index arrives and the makers fill the list; the sheet must
        // outlast that.
        let maker = app.descendants(matching: .any)
            .matching(NSPredicate(format: "label BEGINSWITH '1MORE'")).firstMatch
        _ = maker.waitForExistence(timeout: 30)
        sleep(3)
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = "autoeq-sheet"
        shot.lifetime = .keepAlways
        add(shot)
        XCTAssert(cancel.exists, "Find in AutoEQ closed itself")
        XCTAssert(maker.exists, "the makers never arrived")
    }
}
