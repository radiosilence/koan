import XCTest

/// Each way onto the television, and each way it can go wrong, from a fresh
/// install against a real server. `just tv-signin` runs one route at a time,
/// reinstalling the app between them, with the route in `KOAN_SIGNIN_ROUTE`:
///
/// - `password`, `apikey`: the account form, typed with the remote's keyboard.
/// - `invite`: an invite link handed to the app, as `koan://join`.
/// - `signout`: signs in with a password, then out from Settings.
/// - `revoked`: signs in with an API key, revokes it on the server and
///   relaunches; the app must say the server refused its sign-in.
/// - `wrong-password`: the form with a password the server refuses.
/// - `unreachable`: asks an address nothing answers for a code.
/// - `menu`: Menu on the sign-in page, then back into the app.
///
/// The server is `KOAN_SIGNIN_SERVER`; the account `KOAN_SIGNIN_USER` with
/// `KOAN_SIGNIN_SECRET`, its password or API key; the invite `KOAN_SIGNIN_INVITE`.
/// Every step is screenshotted.
@MainActor
final class TVSignInTests: XCTestCase {
    private let app = XCUIApplication()
    private let remote = XCUIRemote.shared
    private let env = ProcessInfo.processInfo.environment

    func testRoute() throws {
        guard let route = env["KOAN_SIGNIN_ROUTE"] else { throw XCTSkip("no route given") }
        // Nothing here needs the local network; see `TVWalkTests`.
        app.launchEnvironment["KOAN_DEVICES__NEARBY"] = "false"
        app.launch()
        XCTAssertTrue(app.buttons["Get a Code"].waitForExistence(timeout: 30), "a fresh install starts signed out")
        snap("\(route)-01-signed-out")

        switch route {
        case "password": try signInWithForm(apiKey: false, expectSuccess: true, route: route)
        case "apikey": try signInWithForm(apiKey: true, expectSuccess: true, route: route)
        case "wrong-password": try signInWithForm(apiKey: false, expectSuccess: false, route: route)
        case "signout":
            try signInWithForm(apiKey: false, expectSuccess: true, route: route)
            signOut(route: route)
        case "revoked":
            try signInWithForm(apiKey: true, expectSuccess: true, route: route)
            try revokeAndRelaunch(route: route)
        case "invite": try joinInvite(route: route)
        case "unreachable": unreachable(route: route)
        case "menu": menu(route: route)
        default: XCTFail("unknown route \(route)")
        }
    }

    // MARK: - Routes

    private func signInWithForm(apiKey: Bool, expectSuccess: Bool, route: String) throws {
        let server = try XCTUnwrap(env["KOAN_SIGNIN_SERVER"])
        let user = try XCTUnwrap(env["KOAN_SIGNIN_USER"])
        let secret = expectSuccess ? try XCTUnwrap(env["KOAN_SIGNIN_SECRET"]) : "not-the-password"

        XCTAssertTrue(focus(app.buttons["Use a Password or API Key"]))
        remote.press(.select)
        XCTAssertTrue(focus(app.buttons["Server"]), "the account form's settings open")
        remote.press(.select)

        type(server, into: app.textFields["server-url"])
        type(user, into: app.textFields["username"])
        if apiKey {
            let picker = app.buttons["Sign in with"]
            XCTAssertTrue(focus(picker))
            remote.press(.select)
            XCTAssertTrue(focus(app.buttons["API key"]))
            remote.press(.select)
            sleep(1)
        }
        type(secret, into: app.secureTextFields["secret"])
        snap("\(route)-02-form")
        XCTAssertTrue(focus(app.buttons["Sign In"]))
        remote.press(.select)

        if expectSuccess {
            XCTAssertTrue(app.buttons["Library"].waitForExistence(timeout: 30), "signed in, the app is shown")
            sleep(8)
            snap("\(route)-03-signed-in")
        } else {
            let refused = app.staticTexts.containing(
                NSPredicate(format: "label CONTAINS[c] %@ OR label CONTAINS[c] %@", "password", "refused")
            ).firstMatch
            XCTAssertTrue(refused.waitForExistence(timeout: 20), "the form says the password was refused")
            snap("\(route)-03-refused")
            XCTAssertFalse(app.buttons["Library"].exists, "still signed out")
            // The way back: the form is still there to correct, and Menu
            // returns to the sign-in page.
            XCTAssertTrue(app.buttons["Sign In"].exists)
            remote.press(.menu)
            sleep(1)
            remote.press(.menu)
            XCTAssertTrue(app.buttons["Get a Code"].waitForExistence(timeout: 10), "back on the sign-in page")
            snap("\(route)-04-back")
        }
    }

    private func signOut(route: String) {
        XCTAssertTrue(focus(app.buttons["Settings"]), "the Settings tab")
        remote.press(.select)
        sleep(1)
        remote.press(.down)
        XCTAssertTrue(focus(app.buttons["Server"]))
        remote.press(.select)
        XCTAssertTrue(focus(app.buttons["Sign Out"]))
        remote.press(.select)
        let keep = app.buttons["Sign Out, Keep Them in the Library"]
        XCTAssertTrue(focus(keep), "sign-out asks what to keep")
        snap("\(route)-04-confirm")
        remote.press(.select)
        XCTAssertTrue(app.buttons["Get a Code"].waitForExistence(timeout: 20), "signed out, the sign-in page is back")
        snap("\(route)-05-signed-out")
    }

    private func revokeAndRelaunch(route: String) throws {
        let server = try XCTUnwrap(env["KOAN_SIGNIN_SERVER"])
        let key = try XCTUnwrap(env["KOAN_SIGNIN_SECRET"])
        var url = try XCTUnwrap(URLComponents(string: server + "/rest/koanRevokeKey"))
        url.queryItems = [
            .init(name: "apiKey", value: key), .init(name: "v", value: "1.16.1"),
            .init(name: "c", value: "tv-signin-test"), .init(name: "f", value: "json"),
        ]
        let done = expectation(description: "revoked")
        var body = ""
        URLSession.shared.dataTask(with: try XCTUnwrap(url.url)) { data, _, _ in
            body = data.map { String(decoding: $0, as: UTF8.self) } ?? ""
            done.fulfill()
        }.resume()
        wait(for: [done], timeout: 10)
        XCTAssertTrue(body.contains("\"status\":\"ok\""), "the key is revoked: \(body)")

        app.terminate()
        app.launch()
        let refused = app.staticTexts.containing(
            NSPredicate(format: "label CONTAINS %@", "refused kōan's sign-in")
        ).firstMatch
        XCTAssertTrue(refused.waitForExistence(timeout: 60), "the app says the server refused its sign-in")
        snap("\(route)-04-refused")
    }

    private func joinInvite(route: String) throws {
        let link = try XCTUnwrap(env["KOAN_SIGNIN_INVITE"])
        let fragment = try XCTUnwrap(URL(string: link)?.fragment)
        app.open(try XCTUnwrap(URL(string: "koan://join#\(fragment)")))
        XCTAssertTrue(app.buttons["Library"].waitForExistence(timeout: 30), "joined, the app is shown")
        sleep(8)
        snap("\(route)-02-joined")
    }

    private func unreachable(route: String) {
        type("http://koan-unreachable.invalid", into: app.textFields["pair-server"])
        XCTAssertTrue(focus(app.buttons["Get a Code"]))
        remote.press(.select)
        snap("\(route)-02-connecting")
        let problem = app.staticTexts.containing(
            NSPredicate(format: "label BEGINSWITH %@", "Could not sign in this way")
        ).firstMatch
        XCTAssertTrue(problem.waitForExistence(timeout: 90), "the page says the server could not be reached")
        XCTAssertFalse(problem.label.contains("password or API key"), "an unreachable server is not called one without pairing")
        XCTAssertTrue(app.buttons["Get a Code"].exists, "and the button is back to try again")
        snap("\(route)-03-unreachable")
    }

    private func menu(route: String) {
        remote.press(.menu)
        sleep(2)
        snap("\(route)-02-after-menu")
        app.activate()
        XCTAssertTrue(app.buttons["Get a Code"].waitForExistence(timeout: 10), "the sign-in page is still there")
        XCTAssertFalse(app.buttons["Library"].exists, "and not an empty app behind it")
        snap("\(route)-03-returned")
    }

    // MARK: - The remote

    /// Select a field, type into the keyboard it opens, and come back out.
    /// Menu leaves a television's keyboard with what was typed; its return
    /// key moves on to the next field instead, keyboard and all.
    private func type(_ text: String, into field: XCUIElement) {
        XCTAssertTrue(focus(field), "\(field) can be focused")
        remote.press(.select)
        sleep(1)
        // Whatever the field held: an address the app already knew.
        let existing = (field.value as? String) ?? ""
        let clear = String(repeating: XCUIKeyboardKey.delete.rawValue, count: existing.count)
        app.typeText(clear + text)
        sleep(1)
        remote.press(.menu)
        sleep(1)
    }

    /// Move until `element` has focus: along the row, then down and up the
    /// page. Counting presses breaks whenever a control comes or goes.
    @discardableResult
    private func focus(_ element: XCUIElement) -> Bool {
        guard element.waitForExistence(timeout: 10) else { return false }
        for direction in [XCUIRemote.Button.down, .up, .right, .left] {
            for _ in 0..<12 {
                if element.hasFocus { return true }
                remote.press(direction)
                Thread.sleep(forTimeInterval: 0.4)
            }
        }
        return element.hasFocus
    }

    private func snap(_ name: String) {
        let shot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }
}
