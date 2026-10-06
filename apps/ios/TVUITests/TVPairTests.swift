import XCTest

/// Signs the television in by pairing, end to end, against a real server.
///
/// The app starts signed out with only the server's address, asks for a code,
/// and waits for the sign-in page to give way to the app. Given an account,
/// this test approves the code the way a phone would — `koanPairApprove` with
/// its credentials — which is `just tv-pair`; without one something else
/// approves it, which is `just tv-pair-qr`, where a phone scans the code off
/// the screen. Both run against a local `koan` started for the purpose.
/// Screenshots of each step are kept.
@MainActor
final class TVPairTests: XCTestCase {
    func testPairing() throws {
        let env = ProcessInfo.processInfo.environment
        guard let server = env["KOAN_PAIR_SERVER"]
        else { throw XCTSkip("KOAN_PAIR_SERVER names the server") }

        let app = XCUIApplication()
        app.launchEnvironment["KOAN_REMOTE__URL"] = server
        // Nothing here needs the local network; see `TVWalkTests`.
        app.launchEnvironment["KOAN_DEVICES__NEARBY"] = "false"
        app.launch()

        let getCode = app.buttons["Get a Code"]
        XCTAssertTrue(getCode.waitForExistence(timeout: 20), "the sign-in page shows")
        snap("01-sign-in")
        // From the address field, along to the button beside it.
        for _ in 0..<4 where !getCode.hasFocus {
            XCUIRemote.shared.press(.right)
        }
        XCUIRemote.shared.press(.select)

        let code = app.staticTexts.matching(
            NSPredicate(format: "label MATCHES %@", "^[0-9A-Z]{4}-[0-9A-Z]{4}$")
        ).firstMatch
        XCTAssertTrue(code.waitForExistence(timeout: 15), "a code is shown")
        snap("02-code")

        if let user = env["KOAN_PAIR_USER"], let password = env["KOAN_PAIR_PASSWORD"] {
            try approve(code.label, on: server, as: user, password: password)
        }

        XCTAssertTrue(
            app.buttons["Library"].waitForExistence(timeout: 240),
            "the app is shown once the pairing is approved"
        )
        sleep(3)
        snap("03-signed-in")
    }

    private func approve(_ code: String, on server: String, as user: String, password: String) throws {
        var url = URLComponents(string: server + "/rest/koanPairApprove")!
        url.queryItems = [
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
