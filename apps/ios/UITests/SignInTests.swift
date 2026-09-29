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
        app.launch()

        let settings = app.tabBars.buttons["Settings"]
        XCTAssert(settings.waitForExistence(timeout: 10))
        settings.tap()
        app.buttons["Server"].firstMatch.tap()

        if !app.buttons["Sign Out"].waitForExistence(timeout: 2) {
            // SwiftUI exposes a form's fields by their placeholder, not their label.
            fill(field(app.textFields, "https://music.example.com"), url)
            fill(field(app.textFields, "your account"), user)
            fill(field(app.secureTextFields, "hunter2"), password)
            app.buttons["Sign In"].tap()
        }
        XCTAssert(app.buttons["Sign Out"].waitForExistence(timeout: 30), "not signed in")

        // Signing in starts a sync; the albums are what the reviewer needs.
        app.tabBars.buttons["Library"].tap()
        app.buttons["Albums"].firstMatch.tap()
        XCTAssert(app.images.firstMatch.waitForExistence(timeout: 180), "no albums after sync")
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
