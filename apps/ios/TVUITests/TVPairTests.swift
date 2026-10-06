import XCTest

/// Signs the television in by pairing, end to end, against a real server.
///
/// The app starts signed out with only the server's address, asks for a code,
/// and waits for the sign-in page to give way to the app. Given an account,
/// this test approves the code the way a phone would — `koanPairApprove` with
/// its credentials — which is `just tv-pair`; without one something else
/// approves it, which is `just tv-pair-qr`, where a phone scans the code off
/// the screen. `KOAN_PAIR_OUTCOME` asks for the other endings instead:
/// `decline`, which turns the code away, or `expire`, which approves nothing
/// and waits for the code to lapse; either must say what happened and offer
/// the code again. With `KOAN_PAIR_DISCOVER`, the TV is given no address and
/// leaves the local network on: it must find the server announced by kōan on
/// another device, which is `just tv-discover`. Screenshots of each step are
/// kept.
@MainActor
final class TVPairTests: XCTestCase {
    func testPairing() throws {
        let env = ProcessInfo.processInfo.environment
        guard let server = env["KOAN_PAIR_SERVER"]
        else { throw XCTSkip("KOAN_PAIR_SERVER names the server") }

        let discover = env["KOAN_PAIR_DISCOVER"] != nil
        let app = XCUIApplication()
        // Silent, and blind to UPnP renderers: tests share a machine with its owner.
        app.launchEnvironment["KOAN_PLAYBACK__MUTED"] = "true"
        app.launchEnvironment["KOAN_PLAYBACK__RENDERERS"] = "false"
        if !discover {
            app.launchEnvironment["KOAN_REMOTE__URL"] = server
            // Nothing here needs the local network; see `TVWalkTests`.
            app.launchEnvironment["KOAN_DEVICES__NEARBY"] = "false"
        }
        app.launch()

        let getCode = app.buttons[any: "Get a Code"]
        XCTAssertTrue(getCode.waitForExistence(timeout: 20), "the sign-in page shows")
        snap("01-sign-in")
        if discover {
            // Announced by the other device, and offered above the address.
            let host = URLComponents(string: server).map { "\($0.host ?? ""):\($0.port ?? 0)" } ?? server
            let offered = app.buttons.matching(identifier: "found-server")
                .containing(NSPredicate(format: "label CONTAINS %@", host)).firstMatch
            let anyOffered = app.buttons.matching(identifier: "found-server").firstMatch
            XCTAssertTrue(anyOffered.waitForExistence(timeout: 90), "a server another device is signed in to is offered")
            snap("01b-found")
            XCTAssertTrue(offered.exists || anyOffered.label.contains(host), "the one the other device announced: \(host)")
            // The address field has focus on arrival; the server found sits above it.
            XCUIRemote.shared.press(.up)
            XCUIRemote.shared.press(.select)
        } else {
            // From the address field, along to the button beside it.
            for _ in 0..<4 where !getCode.hasFocus {
                XCUIRemote.shared.press(.right)
            }
            XCUIRemote.shared.press(.select)
        }

        let code = app.staticTexts.matching(
            NSPredicate(format: "label MATCHES %@", "^[0-9A-Z]{4}-[0-9A-Z]{4}$")
        ).firstMatch
        XCTAssertTrue(code.waitForExistence(timeout: 15), "a code is shown")
        snap("02-code")

        let outcome = env["KOAN_PAIR_OUTCOME"] ?? "approve"
        if outcome != "expire", let user = env["KOAN_PAIR_USER"], let password = env["KOAN_PAIR_PASSWORD"] {
            try approve(code.label, on: server, as: user, password: password, decline: outcome == "decline")
        }
        if outcome != "approve" {
            let said = outcome == "decline" ? "declined" : "expired"
            let problem = app.staticTexts.containing(NSPredicate(format: "label CONTAINS %@", said)).firstMatch
            XCTAssertTrue(problem.waitForExistence(timeout: 120), "the page says the code was \(said)")
            XCTAssertFalse(problem.label.contains("password or API key"), "a \(said) code is not a server without pairing")
            XCTAssertTrue(app.buttons[any: "Get a Code"].exists, "and offers a code again")
            snap("03-\(said)")
            return
        }

        XCTAssertTrue(
            app.buttons[any: "Library"].waitForExistence(timeout: 240),
            "the app is shown once the pairing is approved"
        )
        sleep(3)
        snap("03-signed-in")

        // Whose account it is: a TV approved by a read-only account is that
        // account, and nothing more.
        let remote = XCUIRemote.shared
        XCTAssertTrue(focus(app.buttons[any: "Settings"]))
        remote.press(.select)
        sleep(1)
        remote.press(.down)
        XCTAssertTrue(focus(app.buttons[any: "Server"]))
        remote.press(.select)
        sleep(2)
        snap("04-account")
    }

    /// Move until `element` has focus: down and up the page, then along.
    @discardableResult
    private func focus(_ element: XCUIElement) -> Bool {
        guard element.waitForExistence(timeout: 10) else { return false }
        for direction in [XCUIRemote.Button.down, .up, .right, .left] {
            for _ in 0..<12 {
                if element.hasFocus { return true }
                XCUIRemote.shared.press(direction)
                Thread.sleep(forTimeInterval: 0.4)
            }
        }
        return element.hasFocus
    }

    private func approve(_ code: String, on server: String, as user: String, password: String, decline: Bool) throws {
        var url = URLComponents(string: server + "/rest/koanPairApprove")!
        url.queryItems = [
            .init(name: "decline", value: decline ? "true" : "false"),
            .init(name: "pair", value: code), .init(name: "u", value: user),
            .init(name: "p", value: password), .init(name: "v", value: "1.16.1"),
            .init(name: "c", value: "tv-pair-test"), .init(name: "f", value: "json"),
        ]
        let done = expectation(description: "approved")
        var body = ""
        URLSession.shared.dataTask(with: url.url!) { data, _, _ in
            body = data.map { String(decoding: $0, as: UTF8.self) } ?? ""
            done.fulfill()
        }.resume()
        wait(for: [done], timeout: 10)
        XCTAssertTrue(body.contains("\"status\":\"ok\""), "approval accepted: \(body)")
    }

    private func snap(_ name: String) {
        let shot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }
}
