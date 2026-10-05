import XCTest

/// The Downloaded page and offline mode, one step at a time, each screen kept
/// as a screenshot. Driven from outside, which is where the server is started
/// and stopped: `KOAN_OFFLINE_STEP` names the step, and without it the test is
/// skipped, so `ios-walk` can run the target as a whole.
///
/// - `downloaded`: the Downloaded page.
/// - `manual`: Offline mode turned on in Settings, the Library tab, Albums,
///   a search and the queue, then turned off again.
/// - `cut`: with the server stopped, waits for the app to go offline by
///   itself, then the same screens.
/// - `back`: with the server running again, waits for it to lift.
@MainActor
final class OfflineTests: XCTestCase {
    private let app = XCUIApplication()

    func testOffline() throws {
        guard let step = ProcessInfo.processInfo.environment["KOAN_OFFLINE_STEP"] else {
            throw XCTSkip("no step given")
        }
        app.launch()
        switch step {
        case "signin":
            let env = ProcessInfo.processInfo.environment
            tab("Settings")
            app.buttons["Server"].firstMatch.tap()
            if !app.buttons["Sign Out"].waitForExistence(timeout: 2) {
                fill(app.textFields, "Server URL", env["KOAN_SIGNIN_URL"] ?? "")
                fill(app.textFields, "Username", env["KOAN_SIGNIN_USER"] ?? "")
                fill(app.secureTextFields, "Password", env["KOAN_SIGNIN_PASSWORD"] ?? "")
                // The keyboard covers the button on a phone.
                if app.keyboards.buttons["return"].exists { app.keyboards.buttons["return"].tap() }
                let signIn = app.buttons["Sign In"].firstMatch
                if !signIn.isHittable { app.swipeUp() }
                signIn.tap()
            }
            XCTAssert(app.buttons["Sign Out"].waitForExistence(timeout: 60), "not signed in")
            snap("signed-in")
        case "downloaded":
            tab("Library")
            app.buttons["Downloaded"].firstMatch.tap()
            XCTAssert(app.images.firstMatch.waitForExistence(timeout: 20), "nothing downloaded")
            pause(2)
            snap("downloaded")
        case "manual":
            setOffline(true)
            tab("Library")
            XCTAssert(app.staticTexts["Offline mode is on"].waitForExistence(timeout: 10))
            snap("manual-library")
            shown(narrowedTo: "manual")
            setOffline(false)
            tab("Library")
            XCTAssert(app.staticTexts["Offline mode is on"].waitForNonExistence(timeout: 10))
            snap("manual-off")
        case "cut":
            tab("Library")
            XCTAssert(app.staticTexts["Can't reach your server"].waitForExistence(timeout: 60), "never went offline")
            snap("cut-library")
            shown(narrowedTo: "cut")
        case "back":
            tab("Library")
            XCTAssert(app.staticTexts["Can't reach your server"].waitForNonExistence(timeout: 90), "never came back")
            snap("back-library")
            app.buttons["Albums"].firstMatch.tap()
            pause(2)
            snap("back-albums")
        default:
            XCTFail("no step \(step)")
        }
    }

    /// Albums, a search and the queue, as offline narrows them.
    private func shown(narrowedTo name: String) {
        app.buttons["Albums"].firstMatch.tap()
        pause(2)
        snap("\(name)-albums")
        back()
        tab("Search")
        let field = app.searchFields.firstMatch
        if field.waitForExistence(timeout: 5) {
            field.tap()
            field.typeText("Harmonic")
            pause(2)
            snap("\(name)-search")
            // The keyboard covers the tab bar.
            field.typeText("\n")
            pause(1)
        }
        tab("Queue")
        pause(2)
        snap("\(name)-queue")
    }

    private func setOffline(_ on: Bool) {
        tab("Settings")
        // The tab keeps the page it was left on.
        if !app.switches["Offline mode"].exists { app.buttons["Server"].firstMatch.tap() }
        let toggle = app.switches["Offline mode"].firstMatch
        XCTAssert(toggle.waitForExistence(timeout: 10), "no Offline mode switch")
        if (toggle.value as? String == "1") != on {
            // The label takes the tap on a form's switch; the switch's own
            // frame is the whole row.
            toggle.switches.firstMatch.exists ? toggle.switches.firstMatch.tap() : toggle.tap()
        }
        pause(1)
        back()
    }

    private func fill(_ query: XCUIElementQuery, _ placeholder: String, _ text: String) {
        let field = query.matching(NSPredicate(format: "placeholderValue == %@", placeholder)).firstMatch
        XCTAssert(field.waitForExistence(timeout: 5), "no field \(placeholder)")
        field.tap()
        field.typeText(text)
    }

    /// A phone's tabs are a tab bar; an iPad's are buttons in a bar across the top.
    private func tab(_ name: String) {
        let bar = app.tabBars.buttons[name]
        if bar.waitForExistence(timeout: 10) { bar.tap() } else { app.buttons[name].firstMatch.tap() }
        pause(1)
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
