import XCTest

/// Signs in to a server through Settings, the way App Review does, and waits
/// for the library to arrive.
///
/// `just ios-signin` passes the server and account in; without them the test
/// is skipped, so `ios-walk` can run the target as a whole.
@MainActor
final class SignInTests: XCTestCase {
    func testSignIn() throws {
        let env = ProcessInfo.processInfo.environment
        guard let url = env["KOAN_SIGNIN_URL"], let user = env["KOAN_SIGNIN_USER"],
              let password = env["KOAN_SIGNIN_PASSWORD"]
        else { throw XCTSkip("no server given") }

        let app = XCUIApplication()
        // Silent, and blind to UPnP renderers: tests share a machine with its owner.
        app.launchEnvironment["KOAN_PLAYBACK__MUTED"] = "true"
        app.launchEnvironment["KOAN_PLAYBACK__RENDERERS"] = "false"
        app.launch()

        tab(app, "Settings")
        app.buttons[any: "Server"].firstMatch.tap()

        if !app.buttons[any: "Sign Out"].waitForExistence(timeout: 2) {
            // SwiftUI exposes a form's fields by their placeholder, not their label.
            fill(field(app.textFields, "Server URL"), url)
            fill(field(app.textFields, "Username"), user)
            fill(field(app.secureTextFields, "Password"), password)
            app.buttons[any: "Sign In"].tap()
        }
        XCTAssert(app.buttons[any: "Sign Out"].waitForExistence(timeout: 30), "not signed in")

        // Signing in starts a sync; the albums are what the reviewer needs.
        tab(app, "Library")
        app.buttons[any: "Albums"].firstMatch.tap()
        XCTAssert(app.images.firstMatch.waitForExistence(timeout: 180), "no albums after sync")
    }

    /// A phone's tabs are the theme's bar, or the platform's tab bar in its
    /// own look; an iPad's are buttons in a bar across the top. The platform's
    /// tab bar shows for a moment at launch before the theme hides it, so it
    /// is asked for only once the theme's bar has had its chance.
    private func tab(_ app: XCUIApplication, _ name: String) {
        let theme = app.otherElements["koan-bar"].buttons[any: name]
        let bar = app.tabBars.buttons[any: name]
        if theme.waitForExistence(timeout: 10) {
            theme.tap()
        } else if bar.exists {
            bar.tap()
        } else {
            app.buttons[any: name].firstMatch.tap()
        }
    }

    private func field(_ query: XCUIElementQuery, _ placeholder: String) -> XCUIElement {
        query.matching(NSPredicate(format: "placeholderValue == %@", placeholder)).firstMatch
    }

    private func fill(_ field: XCUIElement, _ text: String) {
        XCTAssert(field.waitForExistence(timeout: 5), "no field \(field)")
        field.tap()
        field.typeText(text)
    }
}
